//! The download list and settings, kept in one small JSON file next to the engine's state.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting for a free slot (see `Settings::max_simultaneous`).
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Record {
    pub id: u64,
    pub url: String,
    pub dir: PathBuf,
    pub name: String,
    pub total: Option<u64>,
    pub downloaded: u64,
    pub status: Status,
    pub error: Option<String>,
    pub path: Option<PathBuf>,
    /// Request headers and cookie captured by the browser extension, replayed on every (re)start.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub cookie: Option<String>,
    /// A name chosen by the browser; overrides the one the server suggests.
    #[serde(default)]
    pub file_name: Option<String>,
    /// Connections for this download, if it was given its own number in the new-download dialog.
    #[serde(default)]
    pub segments: Option<usize>,
}

impl Record {
    pub fn new(id: u64, url: String, dir: PathBuf, name: String) -> Self {
        Self { id, url, dir, name, total: None, downloaded: 0, status: Status::Downloading, error: None, path: None, headers: Vec::new(), cookie: None, file_name: None, segments: None }
    }
}

/// One of several links sent together ("Grab all links"): they share one dialog.
#[derive(Serialize, Deserialize, Clone)]
pub struct BatchItem {
    pub url: String,
    pub name: Option<String>,
    pub size: Option<u64>,
}

/// A link waiting in the "new download" dialog. Kept on disk: the browser's own copy was already cancelled, so
/// closing ADM before answering must not lose it.
#[derive(Serialize, Deserialize, Clone)]
pub struct Pending {
    pub id: u64,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub cookie: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    /// The page the link came from, when the browser said so.
    pub page: Option<String>,
    /// Set when several links wait together: `url` is then just the first of them.
    #[serde(default)]
    pub batch: Vec<BatchItem>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)] // a list saved by an older version has no newer fields: they take their defaults
pub struct Settings {
    pub max_connections: usize,
    /// 0 = unlimited.
    pub speed_limit_kib: u64,
    pub download_dir: PathBuf,
    /// Show the "new download" dialog before a link starts (from the browser, or typed in).
    pub ask_before_download: bool,
    /// Save into `Video`, `Music`, ... folders under the download folder, by file type.
    pub sort_by_category: bool,
    /// Sites whose downloads the browser extension leaves to the browser.
    pub blocked_sites: Vec<String>,
    /// Start new downloads in the folder chosen last time instead of always in `download_dir`.
    pub remember_folder: bool,
    /// The folder chosen last time in the "New download" window.
    pub last_folder: Option<PathBuf>,
    /// How many downloads run at once; the rest wait their turn. 0 means no limit.
    pub max_simultaneous: usize,
}

impl Settings {
    /// Where a new download goes unless told otherwise: the folder used last time (if remembering, and it still
    /// exists), else the download folder.
    pub fn default_dir(&self) -> PathBuf {
        match &self.last_folder {
            Some(dir) if self.remember_folder && dir.is_dir() => dir.clone(),
            _ => self.download_dir.clone(),
        }
    }

    /// Notes the folder the user just picked. Does nothing unless remembering is on and the folder exists.
    pub fn remember(&mut self, chosen: &std::path::Path) {
        if self.remember_folder && chosen.is_dir() {
            self.last_folder = Some(chosen.to_path_buf());
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
        Self {
            max_connections: 8,
            speed_limit_kib: 0,
            download_dir: home.map_or_else(|| PathBuf::from("."), |h| h.join("Downloads")),
            ask_before_download: true,
            sort_by_category: false,
            blocked_sites: Vec::new(),
            remember_folder: true,
            last_folder: None,
            max_simultaneous: 3,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Db {
    pub settings: Settings,
    pub records: Vec<Record>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}

/// Folder names a download can be sorted into.
pub const CATEGORIES: [&str; 5] = ["Video", "Music", "Documents", "Compressed", "Programs"];

/// The category folder a file belongs in, or "" for none.
pub fn category_of(file_name: &str) -> &'static str {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let is = |list: &str| list.split(' ').any(|e| e == ext);
    if is("mp4 mkv webm mov avi m4v wmv flv mpg mpeg ts 3gp") {
        "Video"
    } else if is("mp3 m4a aac flac wav ogg opus") {
        "Music"
    } else if is("pdf doc docx xls xlsx ppt pptx epub odt ods odp txt") {
        "Documents"
    } else if is("zip rar 7z tar gz xz bz2 zst tgz iso img dmg cab") {
        "Compressed"
    } else if is("exe msi deb rpm appimage apk xapk pkg msix jar") {
        "Programs"
    } else {
        ""
    }
}

impl Db {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Written to a temp file and renamed, so a crash never leaves a half-written list. It can hold
    /// session cookies, so on Unix only the owner may read it.
    pub fn save(&self, path: &Path) {
        let Ok(json) = serde_json::to_vec(self) else { return };
        let tmp = path.with_extension("json.tmp");
        if write_private(&tmp, &json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(data)
}
