//! End-to-end tests: the real engine against a real local HTTP server that supports ranges, throttling,
//! mid-stream disconnects, and the other ways servers misbehave.

use adm_core::*;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const LAST_MODIFIED: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

/// The file the server serves: every byte is a function of its position, so any misplaced, duplicated or
/// missing byte shows up when the result is checked.
fn byte_at(i: u64) -> u8 {
    (i.wrapping_mul(2654435761) >> 11) as u8 ^ (i >> 16) as u8
}

struct Opts {
    size: u64,
    ranges: bool,
    unknown_length: bool,
    /// Answer an open-ended `bytes=N-` with 200 and the whole file, but honour bounded ranges (miniserve does).
    open_ended_gets_200: bool,
    /// Pause between 32 KiB pieces.
    piece_delay: Duration,
    /// Cut the connection after this many bytes, for the next `fail_times` requests.
    fail_after: u64,
    fail_times: AtomicI64,
    disposition: Option<&'static str>,
    status: u16,
    requests: AtomicUsize,
    /// Largest range start seen: proves a resume asks for the middle of the file.
    max_start: AtomicU64,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            size: 8 << 20,
            ranges: true,
            unknown_length: false,
            open_ended_gets_200: false,
            piece_delay: Duration::ZERO,
            fail_after: 0,
            fail_times: AtomicI64::new(0),
            disposition: None,
            status: 200,
            requests: AtomicUsize::new(0),
            max_start: AtomicU64::new(0),
        }
    }
}

async fn serve(State(o): State<Arc<Opts>>, h: HeaderMap) -> Response {
    o.requests.fetch_add(1, Ordering::SeqCst);
    if o.status != 200 {
        return Response::builder().status(o.status).body(Body::empty()).unwrap();
    }
    let range = h
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()))
        })
        .filter(|_| o.ranges)
        .filter(|(start, end)| !(o.open_ended_gets_200 && *start == 0 && end.is_none()))
        // A changed file: If-Range no longer matches, so the server sends everything again.
        .filter(|_| h.get(header::IF_RANGE).is_none_or(|v| v == LAST_MODIFIED));
    let (start, end) = match range {
        Some((s, e)) => (s, e.unwrap_or(o.size - 1).min(o.size - 1)),
        None => (0, o.size - 1),
    };
    o.max_start.fetch_max(start, Ordering::SeqCst);
    let len = end + 1 - start;

    let sabotage = o.fail_after > 0 && o.fail_times.fetch_sub(1, Ordering::SeqCst) > 0;
    let limit = if sabotage { o.fail_after.min(len) } else { len };
    let delay = o.piece_delay;
    let body = futures_util::stream::unfold(0u64, move |sent| async move {
        if sent >= len {
            return None;
        }
        if sent >= limit {
            // Cut mid-body: the client sees a broken connection.
            return Some((Err(std::io::Error::other("sabotage")), len));
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let n = (32 * 1024).min(limit - sent);
        let chunk: Vec<u8> = (0..n).map(|i| byte_at(start + sent + i)).collect();
        Some((Ok(Bytes::from(chunk)), sent + n))
    });

    let mut r = Response::builder()
        .header(header::LAST_MODIFIED, LAST_MODIFIED)
        .header(header::CONTENT_TYPE, "application/octet-stream");
    if o.ranges {
        r = r.header(header::ACCEPT_RANGES, "bytes");
    }
    if let Some(d) = o.disposition {
        r = r.header(header::CONTENT_DISPOSITION, d);
    }
    if !o.unknown_length {
        r = r.header(header::CONTENT_LENGTH, len);
    }
    if range.is_some() {
        r = r
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{}", o.size));
    }
    r.body(Body::from_stream(body)).unwrap()
}

async fn spawn_server(o: Opts) -> (SocketAddr, Arc<Opts>) {
    let o = Arc::new(o);
    let app = Router::new().fallback(serve).with_state(o.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, o)
}

fn engine(state: &Path) -> Engine {
    let mut cfg = EngineConfig::new(state);
    cfg.read_timeout = Duration::from_secs(5);
    Engine::new(cfg).unwrap()
}

fn assert_exact(path: &Path, size: u64) {
    let data = std::fs::read(path).unwrap();
    assert_eq!(data.len() as u64, size, "size");
    let bad = data.iter().enumerate().position(|(i, &b)| b != byte_at(i as u64));
    assert_eq!(bad, None, "first wrong byte");
}

struct Dirs {
    state: tempfile::TempDir,
    out: tempfile::TempDir,
}
fn dirs() -> Dirs {
    Dirs { state: tempfile::tempdir().unwrap(), out: tempfile::tempdir().unwrap() }
}

fn visible_files(dir: &Path) -> Vec<String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn segmented_download_is_byte_exact() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { size: 16 * 1024 * 1024 + 12345, ..Default::default() }).await;
    let url = format!("http://{addr}/files/data.bin");
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, url, d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!("not completed") };
    assert_eq!(f.path.file_name().unwrap(), "data.bin");
    assert_exact(&f.path, o.size);
    assert!(o.requests.load(Ordering::SeqCst) > 1, "should have used several connections");
    assert!(visible_files(d.state.path()).is_empty(), "state file is removed after completion");
    // The hidden part file was renamed into place, not left behind.
    assert_eq!(std::fs::read_dir(d.out.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn splits_up_to_the_configured_segment_count() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { piece_delay: Duration::from_millis(2), ..Default::default() }).await;
    let mut req = DownloadRequest::new(1, format!("http://{addr}/x.bin"), d.out.path());
    req.max_segments = Some(4);
    let (h, _rx) = engine(d.state.path()).start(req);
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_exact(&f.path, o.size);
    let n = o.requests.load(Ordering::SeqCst);
    assert!((4..=8).contains(&n), "expected about 4 connections, saw {n}");
}

#[tokio::test]
async fn server_without_range_support_uses_one_stream() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { ranges: false, size: 3 << 20, ..Default::default() }).await;
    let (h, mut rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/plain.bin"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_exact(&f.path, o.size);
    // The download itself, plus one 1-byte request that confirmed the server really ignores ranges.
    assert_eq!(o.requests.load(Ordering::SeqCst), 2);
    let Ok(Event::Init(init)) = rx.recv().await else { panic!("expected Init first") };
    assert!(!init.resumable);
}

#[tokio::test]
async fn stream_of_unknown_length() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { ranges: false, unknown_length: true, size: 2_500_000, ..Default::default() }).await;
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/live.bin"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_exact(&f.path, o.size);
}

#[tokio::test]
async fn empty_file() {
    let d = dirs();
    let (addr, _o) = spawn_server(Opts { ranges: false, size: 1, ..Default::default() }).await;
    // A one-byte file is the smallest the test server can describe; it still exercises the tiny path.
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/one"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_exact(&f.path, 1);
}

#[tokio::test]
async fn survives_dropped_connections() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts {
        size: 6 << 20,
        // A pause between pieces lets hyper flush the headers before the cut, like a real server dying mid-body.
        piece_delay: Duration::from_millis(1),
        fail_after: 300_000,
        fail_times: AtomicI64::new(5),
        ..Default::default()
    })
    .await;
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/flaky.bin"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!("flaky server should still complete") };
    assert_exact(&f.path, o.size);
}

#[tokio::test]
async fn pause_then_resume_continues_from_saved_progress() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { size: 24 << 20, piece_delay: Duration::from_millis(25), ..Default::default() }).await;
    let url = format!("http://{addr}/big.bin");
    let eng = engine(d.state.path());

    let (h, mut rx) = eng.start(DownloadRequest::new(42, url.clone(), d.out.path()));
    loop {
        match rx.recv().await.unwrap() {
            Event::Progress(p) if p.downloaded > 3 << 20 => break,
            Event::Completed(_) | Event::Failed(_) => panic!("finished before it could be paused"),
            _ => {}
        }
    }
    h.pause();
    assert!(matches!(h.wait().await, Outcome::Paused));
    assert!(state_exists(d.state.path(), 42), "pausing saves the state");
    assert!(visible_files(d.out.path()).is_empty(), "nothing published yet");

    o.max_start.store(0, Ordering::SeqCst);
    let (h, _rx) = eng.start(DownloadRequest::new(42, url, d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!("resume should complete") };
    assert_exact(&f.path, o.size);
    assert!(o.max_start.load(Ordering::SeqCst) > 0, "resume must request a later part of the file");
}

fn state_exists(dir: &Path, id: u64) -> bool {
    dir.join(format!("{id}.state")).exists()
}

#[tokio::test]
async fn speed_limit_is_respected() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { size: 1 << 20, ..Default::default() }).await;
    let eng = engine(d.state.path());
    eng.speed_limiter().set_limit(400 * 1024); // 400 KiB/s for 1 MiB: about 2.5 s
    let t = Instant::now();
    let (h, _rx) = eng.start(DownloadRequest::new(1, format!("http://{addr}/limited.bin"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    let took = t.elapsed();
    assert_exact(&f.path, o.size);
    assert!(took >= Duration::from_millis(1800), "finished in {took:?}, limit not applied");
    assert!(took < Duration::from_secs(8), "finished in {took:?}, far slower than the limit");
}

#[tokio::test]
async fn name_comes_from_content_disposition() {
    let d = dirs();
    let (addr, _o) = spawn_server(Opts {
        size: 1 << 16,
        disposition: Some(r#"attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.bin"#),
        ..Default::default()
    })
    .await;
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/download?id=9"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_eq!(f.path.file_name().unwrap(), "résumé.bin");
}

#[tokio::test]
async fn never_overwrites_an_existing_file() {
    let d = dirs();
    std::fs::write(d.out.path().join("same.bin"), b"precious").unwrap();
    let (addr, o) = spawn_server(Opts { size: 1 << 16, ..Default::default() }).await;
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/same.bin"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_eq!(f.path.file_name().unwrap(), "same (1).bin");
    assert_exact(&f.path, o.size);
    assert_eq!(std::fs::read(d.out.path().join("same.bin")).unwrap(), b"precious");
}

#[tokio::test]
async fn http_error_fails_fast() {
    let d = dirs();
    let (addr, _o) = spawn_server(Opts { status: 404, ..Default::default() }).await;
    let (h, _rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/missing"), d.out.path()));
    assert!(matches!(h.wait().await, Outcome::Failed(DownloadError::InvalidResponse)));
}

#[tokio::test]
async fn events_report_init_progress_and_completion() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { size: 4 << 20, piece_delay: Duration::from_millis(2), ..Default::default() }).await;
    let (h, mut rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/ev.bin"), d.out.path()));
    let (mut init, mut progress, mut done) = (false, false, false);
    while let Ok(ev) = rx.recv().await {
        match ev {
            Event::Init(i) => {
                assert_eq!(i.total_size, Some(o.size));
                assert!(i.resumable);
                assert_eq!(i.file_name, "ev.bin");
                init = true;
            }
            Event::Progress(p) => {
                assert!(init, "Init comes first");
                assert!(p.downloaded <= o.size);
                progress = true;
            }
            Event::Completed(_) => {
                done = true;
                break;
            }
            Event::Failed(e) => panic!("failed: {e}"),
            Event::Paused => panic!("paused"),
        }
    }
    assert!(init && progress && done);
    let _ = h.wait().await;
}

#[tokio::test]
async fn splits_even_when_the_probe_gets_200() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts {
        open_ended_gets_200: true,
        size: 8 << 20,
        piece_delay: Duration::from_millis(2),
        ..Default::default()
    })
    .await;
    let (h, mut rx) = engine(d.state.path()).start(DownloadRequest::new(1, format!("http://{addr}/odd.bin"), d.out.path()));
    let Outcome::Completed(f) = h.wait().await else { panic!() };
    assert_exact(&f.path, o.size);
    assert!(o.requests.load(Ordering::SeqCst) > 2, "bounded ranges work, so it should use several connections");
    let Ok(Event::Init(init)) = rx.recv().await else { panic!("expected Init first") };
    assert!(init.resumable);
}

#[tokio::test]
async fn probe_reports_name_size_and_resumability_without_downloading() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts {
        size: 50 << 20,
        disposition: Some(r#"attachment; filename="Holiday Clips.zip""#),
        ..Default::default()
    })
    .await;
    let req = DownloadRequest::new(1, format!("http://{addr}/download?id=9"), d.out.path());
    let info = engine(d.state.path()).probe(&req).await.unwrap();
    assert_eq!(info.file_name, "Holiday Clips.zip");
    assert_eq!(info.size, Some(o.size));
    assert!(info.resumable);
    assert_eq!(info.content_type.as_deref(), Some("application/octet-stream"));
    assert_eq!(o.requests.load(Ordering::SeqCst), 1, "one small request");
    assert_eq!(o.max_start.load(Ordering::SeqCst), 0);
    assert!(visible_files(d.out.path()).is_empty() && std::fs::read_dir(d.out.path()).unwrap().count() == 0, "nothing is written");
}

#[tokio::test]
async fn probe_of_a_server_without_ranges_and_of_errors() {
    let d = dirs();
    let (addr, o) = spawn_server(Opts { ranges: false, size: 3 << 20, ..Default::default() }).await;
    let info = engine(d.state.path()).probe(&DownloadRequest::new(1, format!("http://{addr}/plain.bin"), d.out.path())).await.unwrap();
    assert_eq!((info.file_name.as_str(), info.size, info.resumable), ("plain.bin", Some(o.size), false));

    let (addr, _o) = spawn_server(Opts { status: 404, ..Default::default() }).await;
    let err = engine(d.state.path()).probe(&DownloadRequest::new(1, format!("http://{addr}/missing"), d.out.path())).await;
    assert!(matches!(err, Err(DownloadError::InvalidResponse)));

    // Nothing listening at all.
    let err = engine(d.state.path()).probe(&DownloadRequest::new(1, "http://127.0.0.1:1/x", d.out.path())).await;
    assert!(matches!(err, Err(DownloadError::Network)));
}

#[tokio::test]
async fn probe_honours_a_name_chosen_by_the_caller() {
    let d = dirs();
    let (addr, _o) = spawn_server(Opts { size: 1 << 16, ..Default::default() }).await;
    let mut req = DownloadRequest::new(1, format!("http://{addr}/whatever.bin"), d.out.path());
    req.file_name = Some("sub/chosen name.mkv".into());
    assert_eq!(engine(d.state.path()).probe(&req).await.unwrap().file_name, "chosen name.mkv");
}
