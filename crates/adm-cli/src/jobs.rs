//! Background downloads for the MCP server: each one is a job with an id that can be waited on, polled and
//! cancelled, so a tool call never has to stay open for as long as the file takes.

use adm_core::{DownloadRequest, Engine, EngineConfig, Event, Outcome};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, broadcast, watch};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Running => "running",
            Phase::Completed => "completed",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }
    pub fn is_done(self) -> bool {
        matches!(self, Phase::Completed | Phase::Failed | Phase::Cancelled)
    }
}

/// Everything a status answer needs, copied out so no lock is held while it is sent.
#[derive(Clone)]
pub struct Snap {
    pub phase: Phase,
    pub file_name: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: f64,
    pub eta_secs: Option<u64>,
    pub connections: usize,
    pub resumable: Option<bool>,
    pub path: Option<PathBuf>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    pub started: Instant,
    pub took: Option<Duration>,
}

impl Snap {
    pub fn to_json(&self, id: &str, url: &str) -> Value {
        let mut m = Map::new();
        let mut put = |k: &str, v: Value| {
            if !v.is_null() {
                m.insert(k.into(), v);
            }
        };
        put("job_id", json!(id));
        put("status", json!(self.phase.name()));
        put("url", json!(url));
        put("file_name", json!(self.file_name));
        if self.phase == Phase::Completed {
            put("path", json!(self.path.as_ref().map(|p| p.display().to_string())));
            put("size_bytes", json!(self.size));
            put("sha256", json!(self.sha256));
            let secs = self.took.unwrap_or_default().as_secs_f64();
            put("seconds", json!((secs * 10.0).round() / 10.0));
            if secs > 0.0 {
                put("avg_speed_bytes_per_sec", json!((self.size.unwrap_or(0) as f64 / secs).round()));
            }
        } else {
            put("downloaded_bytes", json!(self.downloaded));
            put("total_bytes", json!(self.total));
            if let Some(t) = self.total.filter(|t| *t > 0) {
                put("percent", json!((self.downloaded as f64 * 1000.0 / t as f64).round() / 10.0));
            }
            if self.phase == Phase::Running {
                put("speed_bytes_per_sec", json!(self.speed.round()));
                put("eta_seconds", json!(self.eta_secs));
                put("active_connections", json!(self.connections));
            }
            // A failed checksum still leaves the file where it is, so say where.
            put("path", json!(self.path.as_ref().map(|p| p.display().to_string())));
            put("sha256", json!(self.sha256));
        }
        put("resumable", json!(self.resumable));
        put("error", json!(self.error));
        Value::Object(m)
    }
}

pub struct Job {
    pub id: String,
    pub url: String,
    pub dest: PathBuf,
    req_id: u64,
    tx: watch::Sender<Snap>,
    cancel: watch::Sender<bool>,
}

impl Job {
    pub fn snap(&self) -> Snap {
        self.tx.borrow().clone()
    }
    pub fn json(&self) -> Value {
        self.snap().to_json(&self.id, &self.url)
    }
    fn update(&self, f: impl FnOnce(&mut Snap)) {
        self.tx.send_modify(f);
    }
}

pub struct Request {
    pub url: String,
    pub dest: PathBuf,
    pub file_name: Option<String>,
    pub headers: Vec<(String, String)>,
    pub connections: Option<usize>,
    pub sha256: Option<String>,
}

pub struct Manager {
    engine: Engine,
    jobs: Mutex<Vec<Arc<Job>>>,
    slots: Arc<Semaphore>,
    next: AtomicU64,
}

/// Same link and folder give the same id, so a download interrupted earlier (even by the CLI) is continued.
pub fn stable_id(url: &str, dir: &Path) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a
    for b in url.bytes().chain(dir.to_string_lossy().bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("adm").join("state")
}

pub fn human(n: f64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let (mut v, mut i) = (n, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

/// SHA-256 of a file as lowercase hex, read in blocks so a huge file does not fill the memory.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

impl Manager {
    pub fn new(cfg: EngineConfig, max_jobs: usize) -> Result<Arc<Self>, String> {
        let engine = Engine::new(cfg).map_err(|e| e.to_string())?;
        Ok(Arc::new(Self { engine, jobs: Mutex::new(Vec::new()), slots: Arc::new(Semaphore::new(max_jobs.max(1))), next: AtomicU64::new(1) }))
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn find(&self, id: &str) -> Option<Arc<Job>> {
        self.jobs.lock().unwrap().iter().find(|j| j.id == id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<Job>> {
        self.jobs.lock().unwrap().clone()
    }

    /// Starts a download in the background. Asking again for a link that is already being downloaded into the same
    /// folder returns that job instead of starting a second copy.
    pub fn start(self: &Arc<Self>, r: Request) -> Arc<Job> {
        let req_id = stable_id(&r.url, &r.dest);
        {
            let jobs = self.jobs.lock().unwrap();
            if let Some(j) = jobs.iter().find(|j| j.req_id == req_id && !j.snap().phase.is_done()) {
                return j.clone();
            }
        }
        let (tx, _) = watch::channel(Snap {
            phase: Phase::Queued,
            file_name: r.file_name.clone(),
            downloaded: 0,
            total: None,
            speed: 0.0,
            eta_secs: None,
            connections: 0,
            resumable: None,
            path: None,
            size: None,
            sha256: None,
            error: None,
            started: Instant::now(),
            took: None,
        });
        let job = Arc::new(Job {
            id: format!("job-{}", self.next.fetch_add(1, Ordering::Relaxed)),
            url: r.url.clone(),
            dest: r.dest.clone(),
            req_id,
            tx,
            cancel: watch::channel(false).0,
        });
        self.jobs.lock().unwrap().push(job.clone());
        tokio::spawn(run(self.clone(), job.clone(), r));
        job
    }

    /// Stops a job and throws away what it had downloaded. Returns false if there is no such job or it is over.
    pub fn cancel(&self, id: &str) -> bool {
        match self.find(id) {
            Some(j) if !j.snap().phase.is_done() => {
                let _ = j.cancel.send(true);
                true
            }
            _ => false,
        }
    }

    /// Waits until the job is over or `wait` has passed, whichever comes first. `on_tick` is called about once a
    /// second with the latest state, so a caller can report progress while it waits.
    pub async fn wait(&self, job: &Arc<Job>, wait: Duration, mut on_tick: impl FnMut(&Snap)) -> Snap {
        let mut rx = job.tx.subscribe();
        let deadline = tokio::time::Instant::now() + wait;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.tick().await;
        loop {
            let s = rx.borrow().clone();
            if s.phase.is_done() || tokio::time::Instant::now() >= deadline {
                return s;
            }
            tokio::select! {
                _ = rx.changed() => {}
                _ = tick.tick() => on_tick(&rx.borrow()),
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }
    }
}

async fn run(m: Arc<Manager>, job: Arc<Job>, r: Request) {
    let mut cancel = job.cancel.subscribe();
    let permit = tokio::select! {
        p = m.slots.clone().acquire_owned() => p.ok(),
        _ = cancel.wait_for(|c| *c) => None,
    };
    let Some(_permit) = permit else {
        job.update(|s| s.phase = Phase::Cancelled);
        return;
    };
    let started = Instant::now();
    job.update(|s| {
        s.phase = Phase::Running;
        s.started = started;
    });

    let mut req = DownloadRequest::new(job.req_id, &r.url, &r.dest);
    req.headers = r.headers;
    req.file_name = r.file_name;
    req.max_segments = r.connections;
    let (handle, mut events) = m.engine.start(req);

    let mut cancelled = false;
    loop {
        tokio::select! {
            _ = cancel.changed(), if !cancelled => {
                if *cancel.borrow() {
                    cancelled = true;
                    handle.pause();
                }
            }
            ev = events.recv() => match ev {
                Ok(Event::Init(i)) => job.update(|s| {
                    s.file_name = Some(i.file_name);
                    s.total = i.total_size;
                    s.resumable = Some(i.resumable);
                }),
                Ok(Event::Progress(p)) => job.update(|s| {
                    s.downloaded = p.downloaded;
                    s.total = p.total.or(s.total);
                    s.speed = p.speed;
                    s.eta_secs = p.eta_secs;
                    s.connections = p.segments.iter().filter(|g| g.downloaded < g.length).count();
                }),
                Ok(Event::Completed(_) | Event::Paused | Event::Failed(_)) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            },
        }
    }

    match handle.wait().await {
        Outcome::Completed(f) => {
            let mut sha = None;
            let mut error = None;
            if let Some(want) = r.sha256 {
                let path = f.path.clone();
                match tokio::task::spawn_blocking(move || sha256_file(&path)).await {
                    Ok(Ok(got)) if got.eq_ignore_ascii_case(&want) => sha = Some(got),
                    Ok(Ok(got)) => {
                        error = Some(format!("checksum mismatch: expected {want}, the file has {got}. The file is kept at {}; do not use it", f.path.display()));
                        sha = Some(got);
                    }
                    _ => error = Some("could not read the file to check its checksum".into()),
                }
            }
            job.update(|s| {
                s.phase = if error.is_some() { Phase::Failed } else { Phase::Completed };
                s.downloaded = f.size;
                s.total = Some(f.size);
                s.size = Some(f.size);
                s.path = Some(f.path);
                s.sha256 = sha;
                s.error = error;
                s.took = Some(started.elapsed());
            });
        }
        Outcome::Paused => {
            // The only thing that pauses a job here is a cancel: forget it, with its part file.
            m.engine.discard(job.req_id, &job.dest);
            job.update(|s| {
                s.phase = Phase::Cancelled;
                s.took = Some(started.elapsed());
            });
        }
        Outcome::Failed(e) => job.update(|s| {
            s.phase = Phase::Failed;
            s.error = Some(e.to_string());
            s.took = Some(started.elapsed());
        }),
    }
}
