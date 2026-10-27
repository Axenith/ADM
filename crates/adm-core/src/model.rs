use crate::error::DownloadError;
use std::path::PathBuf;
use std::time::Duration;

pub type DownloadId = u64;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Parallel connections per download.
    pub max_segments: usize,
    /// Consecutive failed attempts (without progress) before a segment gives up.
    pub max_retries: u32,
    pub connect_timeout: Duration,
    /// A connection that delivers nothing for this long is dropped and retried.
    pub read_timeout: Duration,
    /// `http://`, `https://` or `socks5://` URL. `None` uses the system proxy settings.
    pub proxy: Option<String>,
    pub ignore_cert_errors: bool,
    pub user_agent: String,
    /// Where `<id>.state` files live.
    pub state_dir: PathBuf,
}

impl EngineConfig {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            max_segments: 8,
            max_retries: 10,
            connect_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(30),
            proxy: None,
            ignore_cert_errors: false,
            user_agent: format!("Mozilla/5.0 (compatible; ADM/{})", env!("CARGO_PKG_VERSION")),
            state_dir: state_dir.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub id: DownloadId,
    pub url: String,
    /// Extra request headers (Referer, User-Agent, ...). A `Cookie` header here is merged with `cookie`.
    pub headers: Vec<(String, String)>,
    pub cookie: Option<String>,
    /// Folder the finished file is published in; the part file lives here too so the final rename is atomic.
    pub dest_dir: PathBuf,
    /// Overrides the name derived from the server's response.
    pub file_name: Option<String>,
    /// Overrides `EngineConfig::max_segments` for this download.
    pub max_segments: Option<usize>,
}

impl DownloadRequest {
    pub fn new(id: DownloadId, url: impl Into<String>, dest_dir: impl Into<PathBuf>) -> Self {
        Self {
            id,
            url: url.into(),
            headers: Vec::new(),
            cookie: None,
            dest_dir: dest_dir.into(),
            file_name: None,
            max_segments: None,
        }
    }
}

/// What a link turned out to be, found without downloading it (see `Engine::probe`).
#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub file_name: String,
    pub size: Option<u64>,
    pub final_url: String,
    pub content_type: Option<String>,
    /// The server honours byte ranges, so the download can be split and resumed.
    pub resumable: bool,
}

#[derive(Debug, Clone)]
pub struct InitInfo {
    pub id: DownloadId,
    pub final_url: String,
    pub file_name: String,
    pub total_size: Option<u64>,
    pub content_type: Option<String>,
    pub resumable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentProgress {
    pub offset: u64,
    pub length: u64,
    pub downloaded: u64,
}

#[derive(Debug, Clone)]
pub struct ProgressInfo {
    pub id: DownloadId,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Smoothed, bytes per second.
    pub speed: f64,
    pub eta_secs: Option<u64>,
    pub segments: Vec<SegmentProgress>,
}

#[derive(Debug, Clone)]
pub struct FinalInfo {
    pub id: DownloadId,
    pub size: u64,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum Event {
    Init(InitInfo),
    Progress(ProgressInfo),
    Paused,
    Completed(FinalInfo),
    Failed(DownloadError),
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Completed(FinalInfo),
    Paused,
    Failed(DownloadError),
}
