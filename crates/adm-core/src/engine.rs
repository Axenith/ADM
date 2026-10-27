use crate::error::DownloadError;
use crate::limiter::SpeedLimiter;
use crate::model::*;
use crate::util::{derive_file_name, parse_content_range};
use crate::{coord, error::Result};
use reqwest::header::{ACCEPT_ENCODING, COOKIE, HeaderName, HeaderValue, IF_RANGE, RANGE, RETRY_AFTER, USER_AGENT};
use reqwest::{Client, Response};
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Owns the HTTP client (connection pool, TLS sessions) and the global speed limiter. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    client: Client,
    cfg: Arc<EngineConfig>,
    limiter: Arc<SpeedLimiter>,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Result<Self> {
        // reqwest is built without a bundled crypto provider; ring is the light one.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut b = Client::builder()
            .connect_timeout(cfg.connect_timeout)
            // One TCP connection per segment is the point; HTTP/2 would multiplex them onto one.
            .http1_only()
            .tcp_nodelay(true)
            .pool_idle_timeout(Duration::from_secs(5))
            .danger_accept_invalid_certs(cfg.ignore_cert_errors);
        if let Some(p) = &cfg.proxy {
            b = b.proxy(reqwest::Proxy::all(p).map_err(|_| DownloadError::Internal)?);
        }
        let client = b.build().map_err(|_| DownloadError::Internal)?;
        Ok(Self { client, cfg: Arc::new(cfg), limiter: Arc::new(SpeedLimiter::new(0)) })
    }

    /// The global cap shared by all downloads started from this engine (bytes/second, 0 = unlimited).
    pub fn speed_limiter(&self) -> Arc<SpeedLimiter> {
        self.limiter.clone()
    }

    /// Finds out what a link is (name, size, whether it can be resumed) by asking for its first byte, without
    /// downloading it. Used to fill in the "new download" dialog.
    pub async fn probe(&self, req: &DownloadRequest) -> Result<ProbeInfo> {
        let rb = build_request(&self.client, &self.cfg, req, &req.url, Some((0, Some(0))), None);
        let resp = match tokio::time::timeout(Duration::from_secs(15), rb.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) if is_tls_error(&e) => return Err(DownloadError::Tls),
            _ => return Err(DownloadError::Network),
        };
        let status = resp.status().as_u16();
        if !matches!(status, 200 | 206) {
            return Err(DownloadError::InvalidResponse);
        }
        let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let (size, resumable) = if status == 206 {
            match header(reqwest::header::CONTENT_RANGE).as_deref().and_then(parse_content_range) {
                Some((0, _, Some(total))) => (Some(total), true),
                _ => (None, false),
            }
        } else {
            (resp.content_length(), false)
        };
        Ok(ProbeInfo {
            file_name: derive_file_name(req.file_name.as_deref(), header(reqwest::header::CONTENT_DISPOSITION).as_deref(), &req.url, resp.url().as_str()),
            size,
            final_url: resp.url().to_string(),
            content_type: header(reqwest::header::CONTENT_TYPE),
            resumable,
        })
        // The response is dropped here, which closes the connection: nothing beyond the first byte is read.
    }

    /// Forgets a download that is not running: deletes its saved state and its part file in `dest_dir`.
    pub fn discard(&self, id: DownloadId, dest_dir: &std::path::Path) {
        crate::store::remove(&self.cfg.state_dir, id);
        let _ = std::fs::remove_file(crate::util::part_path(dest_dir, id));
    }

    /// Starts the download, or resumes it if `<state_dir>/<id>.state` exists. Must run inside a tokio runtime.
    /// The receiver is subscribed before anything runs, so no event is missed.
    pub fn start(&self, req: DownloadRequest) -> (DownloadHandle, broadcast::Receiver<Event>) {
        let (events, rx) = broadcast::channel(256);
        let user = CancellationToken::new();
        let shared = Arc::new(Shared {
            max_segments: req.max_segments.unwrap_or(self.cfg.max_segments).clamp(1, 64),
            req,
            cfg: self.cfg.clone(),
            client: self.client.clone(),
            limiter: self.limiter.clone(),
            stop: user.child_token(),
            events: events.clone(),
            file: OnceLock::new(),
            validator: OnceLock::new(),
            ranged: AtomicBool::new(false),
            final_url: Mutex::new(None),
            session_bytes: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            unsynced: AtomicU64::new(0),
        });
        let id = shared.req.id;
        let join = tokio::spawn(coord::run(shared, user.clone()));
        (DownloadHandle { id, events, cancel: user, join }, rx)
    }
}

pub struct DownloadHandle {
    pub id: DownloadId,
    events: broadcast::Sender<Event>,
    cancel: CancellationToken,
    join: JoinHandle<Outcome>,
}

impl DownloadHandle {
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Stops all connections, flushes what was received and saves the state. `wait()` then yields `Paused`.
    /// Call `Engine::start` again with the same id to resume.
    pub fn pause(&self) {
        self.cancel.cancel();
    }

    pub async fn wait(self) -> Outcome {
        self.join.await.unwrap_or(Outcome::Failed(DownloadError::Internal))
    }
}

/// State every segment task and the coordinator share. Anything mutable is an atomic or set-once.
pub(crate) struct Shared {
    pub req: DownloadRequest,
    pub cfg: Arc<EngineConfig>,
    pub client: Client,
    pub limiter: Arc<SpeedLimiter>,
    /// Segments watch this. Cancelled by the user's pause, or by the coordinator when it aborts.
    pub stop: CancellationToken,
    pub events: broadcast::Sender<Event>,
    pub max_segments: usize,
    pub file: OnceLock<Arc<File>>,
    pub validator: OnceLock<Option<String>>,
    /// Whether the server honours byte ranges (decided by the first response).
    pub ranged: AtomicBool,
    /// Where the link led last time; later requests go there directly instead of re-resolving redirects.
    pub final_url: Mutex<Option<String>>,
    /// Bytes written in this run (excludes progress restored from a saved state): drives the speed.
    pub session_bytes: AtomicU64,
    /// All bytes on disk including restored ones: tells a fresh link from one that expired mid-download.
    pub total_bytes: AtomicU64,
    pub unsynced: AtomicU64,
}

pub(crate) enum ConnectErr {
    /// Try again after this long (`None`: the caller's backoff).
    Retry(Option<Duration>),
    Fatal(DownloadError),
    /// A status that retrying cannot fix.
    Status(u16),
}

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

impl Shared {
    pub fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub fn file(&self) -> Arc<File> {
        self.file.get().expect("part file is set before any segment starts").clone()
    }

    fn request(&self, url: &str, range: Option<(u64, Option<u64>)>, if_range: Option<&str>) -> reqwest::RequestBuilder {
        build_request(&self.client, &self.cfg, &self.req, url, range, if_range)
    }

    /// Opens a connection for bytes `start..=end_incl` (or `start..` when open-ended).
    /// With `ranged == false` the whole file is requested and the offsets are ignored.
    pub async fn connect(&self, start: u64, end_incl: Option<u64>, ranged: bool) -> std::result::Result<Response, ConnectErr> {
        let via_final = self.final_url.lock().unwrap().clone();
        let url = via_final.clone().unwrap_or_else(|| self.req.url.clone());
        let validator = self.validator.get().and_then(|v| v.clone());
        // If-Range goes on bounded requests only: if the file changed the server answers 200 with the whole
        // new file, which `bounded && 200` below refuses instead of splicing two versions together.
        let bounded = ranged && end_incl.is_some();
        let rb = self.request(&url, ranged.then_some((start, end_incl)), validator.as_deref().filter(|_| bounded));

        let resp = match tokio::time::timeout(self.cfg.read_timeout.max(self.cfg.connect_timeout), rb.send()).await {
            Err(_) => return Err(ConnectErr::Retry(None)),
            Ok(Err(e)) if is_tls_error(&e) => return Err(ConnectErr::Fatal(DownloadError::Tls)),
            Ok(Err(e)) => {
                tracing::debug!("connect error: {e}");
                return Err(ConnectErr::Retry(None));
            }
            Ok(Ok(r)) => r,
        };
        let status = resp.status().as_u16();

        if matches!(status, 401 | 403 | 410) {
            // A signed redirect target stops working when it expires: go back through the original link,
            // which redirects to a fresh one.
            if via_final.is_some() {
                let mut f = self.final_url.lock().unwrap();
                if f.as_deref() == Some(url.as_str()) {
                    *f = None;
                }
                return Err(ConnectErr::Retry(Some(Duration::ZERO)));
            }
            // The link worked before (bytes are on disk) and is now refused: it expired.
            if self.total_bytes.load(Ordering::Relaxed) > 0 {
                return Err(ConnectErr::Fatal(DownloadError::LinkExpired));
            }
        }
        match status {
            200 | 206 => {}
            408 | 413 | 429 | 500 | 502 | 503 | 504 => {
                let after = resp
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
                return Err(ConnectErr::Retry(after));
            }
            other => return Err(ConnectErr::Status(other)),
        }

        if bounded {
            let end = end_incl.unwrap();
            // 200 is the whole file from byte 0; it must not be written at this segment's offset.
            if status == 200 {
                return Err(ConnectErr::Fatal(DownloadError::ResumeNotSupported));
            }
            let range_ok = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                .is_some_and(|(s, _, _)| s == start);
            if !range_ok {
                return Err(ConnectErr::Fatal(DownloadError::InvalidResponse));
            }
            if resp.content_length().is_some_and(|len| len != end - start + 1) {
                return Err(ConnectErr::Fatal(DownloadError::ResumeNotSupported));
            }
        }
        *self.final_url.lock().unwrap() = Some(resp.url().to_string());
        Ok(resp)
    }
}

/// A GET for `url` with the request's headers and cookies, `Accept-Encoding: identity`, and an optional range.
fn build_request(
    client: &Client,
    cfg: &EngineConfig,
    req: &DownloadRequest,
    url: &str,
    range: Option<(u64, Option<u64>)>,
    if_range: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut rb = client.get(url).header(ACCEPT_ENCODING, "identity");
    let mut cookies: Vec<&str> = Vec::new();
    let mut has_ua = false;
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("cookie") {
            cookies.push(v);
        } else if let (Ok(name), Ok(val)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            has_ua |= name == USER_AGENT;
            rb = rb.header(name, val);
        }
    }
    if !has_ua {
        rb = rb.header(USER_AGENT, &cfg.user_agent);
    }
    cookies.extend(req.cookie.as_deref());
    if !cookies.is_empty() {
        rb = rb.header(COOKIE, cookies.join("; "));
    }
    if let Some((start, end)) = range {
        rb = rb.header(RANGE, match end {
            Some(end) => format!("bytes={start}-{end}"),
            None => format!("bytes={start}-"),
        });
    }
    if let Some(v) = if_range {
        rb = rb.header(IF_RANGE, v);
    }
    rb
}

/// Certificate/hostname failures are not worth retrying; they need the user's decision.
fn is_tls_error(e: &reqwest::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if matches!(err.downcast_ref::<rustls::Error>(), Some(rustls::Error::InvalidCertificate(_))) {
            return true;
        }
        let msg = err.to_string();
        if msg.contains("invalid peer certificate") || msg.contains("certificate verify failed") {
            return true;
        }
        cur = err.source();
    }
    false
}
