// Hide the console window on Windows release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod native_host;
mod server;
mod store;

use adm_core::{DownloadHandle, DownloadRequest, Engine, EngineConfig, Event};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use store::{BatchItem, CATEGORIES, Db, Pending, Record, Settings, Status, category_of, normalize_extensions};
use tauri::{AppHandle, Emitter, Manager, State};

/// What the UI shows for one download. Ids are strings because a u64 does not fit a JS number.
#[derive(Serialize, Clone)]
struct Row {
    id: String,
    name: String,
    url: String,
    status: Status,
    error: Option<String>,
    downloaded: u64,
    total: Option<u64>,
    speed: f64,
    eta: Option<u64>,
    path: Option<String>,
    /// Connections still transferring (running downloads only).
    conns: usize,
    /// One `[start, length, done]` per segment, each as a fraction (0..1) of the file or of the segment.
    segs: Vec<[f32; 3]>,
}

#[derive(Default)]
struct Live {
    speed: f64,
    eta: Option<u64>,
    conns: usize,
    segs: Vec<[f32; 3]>,
}

struct Inner {
    db: Db,
    handles: HashMap<u64, DownloadHandle>,
    live: HashMap<u64, Live>,
    /// Downloads waiting for a free slot, in the order they were asked for.
    queue: VecDeque<u64>,
}

struct App {
    engine: Engine,
    inner: Mutex<Inner>,
    db_path: PathBuf,
}

type Shared = Arc<App>;

impl Inner {
    fn row(&self, id: u64) -> Option<Row> {
        let r = self.db.records.iter().find(|r| r.id == id)?;
        let live = self.live.get(&id);
        Some(Row {
            id: r.id.to_string(),
            name: r.name.clone(),
            url: r.url.clone(),
            status: r.status,
            error: r.error.clone(),
            downloaded: r.downloaded,
            total: r.total,
            speed: live.map_or(0.0, |l| l.speed),
            eta: live.and_then(|l| l.eta),
            path: r.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            conns: live.map_or(0, |l| l.conns),
            segs: live.map(|l| l.segs.clone()).unwrap_or_default(),
        })
    }

    fn record_mut(&mut self, id: u64) -> Option<&mut Record> {
        self.db.records.iter_mut().find(|r| r.id == id)
    }
}

impl App {
    /// Runs `f` on the record under the lock, saves, and tells the UI.
    fn update(&self, app: &AppHandle, id: u64, save: bool, f: impl FnOnce(&mut Record)) {
        let row = {
            let mut g = self.inner.lock().unwrap();
            if let Some(r) = g.record_mut(id) {
                f(r);
            }
            if save {
                g.db.save(&self.db_path);
            }
            g.row(id)
        };
        if let Some(row) = row {
            let _ = app.emit("dl", row);
        }
    }
}

pub(crate) fn new_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    // Under 2^53 so it round-trips through JSON numbers, and strictly increasing so a batch never repeats an id.
    let want = now & ((1 << 53) - 1);
    let mut prev = LAST.load(SeqCst);
    loop {
        let next = want.max(prev + 1);
        match LAST.compare_exchange(prev, next, SeqCst, SeqCst) {
            Ok(_) => return next,
            Err(seen) => prev = seen,
        }
    }
}

/// Starts (or resumes) the engine for a record and pumps its events into the record and the UI.
/// Starts a download now, or puts it in the queue if as many are already running as allowed.
fn spawn_download(app: &AppHandle, st: &Shared, id: u64) {
    let wait = {
        let mut g = st.inner.lock().unwrap();
        let limit = g.db.settings.max_simultaneous;
        if g.handles.contains_key(&id) {
            return;
        }
        let full = limit > 0 && g.handles.len() >= limit;
        if full && !g.queue.contains(&id) {
            g.queue.push_back(id);
        }
        full
    };
    if wait {
        st.update(app, id, true, |r| {
            r.status = Status::Queued;
            r.error = None;
        });
    } else {
        begin_download(app, st, id);
    }
}

/// Starts queued downloads while there are free slots.
fn start_queued(app: &AppHandle, st: &Shared) {
    loop {
        let next = {
            let mut g = st.inner.lock().unwrap();
            let limit = g.db.settings.max_simultaneous;
            if limit > 0 && g.handles.len() >= limit {
                return;
            }
            let Some(id) = g.queue.pop_front() else { return };
            // It may have been paused or removed while it waited.
            g.db.records.iter().any(|r| r.id == id && r.status == Status::Queued).then_some(id)
        };
        if let Some(id) = next {
            begin_download(app, st, id);
        }
    }
}

/// Starts the engine for a record and pumps its events into the record and the UI.
fn begin_download(app: &AppHandle, st: &Shared, id: u64) {
    let (req, already_running) = {
        let g = st.inner.lock().unwrap();
        let Some(r) = g.db.records.iter().find(|r| r.id == id) else { return };
        let mut req = DownloadRequest::new(id, r.url.clone(), r.dir.clone());
        req.max_segments = Some(r.segments.unwrap_or(g.db.settings.max_connections));
        req.headers = r.headers.clone();
        req.cookie = r.cookie.clone();
        req.file_name = r.file_name.clone();
        (req, g.handles.contains_key(&id))
    };
    if already_running {
        return;
    }
    let (handle, mut rx) = st.engine.start(req);
    {
        let mut g = st.inner.lock().unwrap();
        g.handles.insert(id, handle);
        g.live.insert(id, Live::default());
    }
    st.update(app, id, true, |r| {
        r.status = Status::Downloading;
        r.error = None;
    });

    let (app, st) = (app.clone(), st.clone());
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(Event::Init(i)) => st.update(&app, id, true, |r| {
                    r.name = i.file_name;
                    r.total = i.total_size;
                }),
                Ok(Event::Progress(p)) => {
                    let segs = match p.total {
                        Some(t) if t > 0 => p
                            .segments
                            .iter()
                            .map(|s| [(s.offset as f64 / t as f64) as f32, (s.length as f64 / t as f64) as f32, (s.downloaded as f64 / s.length.max(1) as f64) as f32])
                            .collect(),
                        _ => Vec::new(),
                    };
                    let max = st.inner.lock().unwrap().db.settings.max_connections;
                    let conns = p.segments.iter().filter(|s| s.downloaded < s.length).count().min(max);
                    st.inner.lock().unwrap().live.insert(id, Live { speed: p.speed, eta: p.eta_secs, conns, segs });
                    st.update(&app, id, false, |r| {
                        r.downloaded = p.downloaded;
                        r.total = p.total.or(r.total);
                    });
                }
                Ok(Event::Completed(f)) => {
                    st.update(&app, id, true, |r| {
                        r.status = Status::Completed;
                        r.downloaded = f.size;
                        r.total = Some(f.size);
                        r.name = f.path.file_name().map_or(r.name.clone(), |n| n.to_string_lossy().into_owned());
                        r.path = Some(f.path);
                    });
                    break;
                }
                Ok(Event::Paused) => {
                    st.update(&app, id, true, |r| r.status = Status::Paused);
                    break;
                }
                Ok(Event::Failed(e)) => {
                    st.update(&app, id, true, |r| {
                        r.status = Status::Failed;
                        r.error = Some(e.to_string());
                    });
                    break;
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
        let mut g = st.inner.lock().unwrap();
        g.handles.remove(&id);
        g.live.remove(&id);
        let row = g.row(id);
        drop(g);
        if let Some(row) = row {
            let _ = app.emit("dl", row);
        }
        // This one's slot is free: the next in line starts.
        start_queued(&app, &st);
    });
}

#[tauri::command]
fn list(st: State<Shared>) -> Vec<Row> {
    let g = st.inner.lock().unwrap();
    g.db.records.iter().rev().filter_map(|r| g.row(r.id)).collect()
}

#[tauri::command]
fn get_settings(st: State<Shared>) -> Settings {
    st.inner.lock().unwrap().db.settings.clone()
}

#[tauri::command]
fn set_settings(app: AppHandle, st: State<Shared>, mut settings: Settings) {
    let mut g = st.inner.lock().unwrap();
    // A new default folder chosen in Settings wins over the folder remembered from the last download.
    if settings.download_dir != g.db.settings.download_dir {
        settings.last_folder = None;
    }
    st.engine.speed_limiter().set_limit(settings.speed_limit_kib * 1024);
    settings.capture_extensions = normalize_extensions(std::mem::take(&mut settings.capture_extensions));
    g.db.settings = settings;
    g.db.save(&st.db_path);
    drop(g);
    // A higher limit can free slots for downloads that were waiting.
    let shared: Shared = st.inner().clone();
    start_queued(&app, &shared);
}

/// What the browser captured along with a link.
#[derive(Default)]
pub(crate) struct Extra {
    pub headers: Vec<(String, String)>,
    pub cookie: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    /// The page the link came from.
    pub page: Option<String>,
}

/// `https://User@Example.com:8080/a?b` -> `example.com`
fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn guess_name(url: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or("download").split(['?', '#']).next().unwrap_or("download");
    adm_core::sanitize_file_name(last)
}

fn valid_link(url: &str) -> Result<String, String> {
    let url = url.trim().to_string();
    if url.starts_with("http://") || url.starts_with("https://") { Ok(url) } else { Err("Enter an http:// or https:// link".into()) }
}

/// Adds a link to the list and starts it right away. Must run inside the Tokio runtime (`Engine::start` spawns tasks).
fn add_url(app: &AppHandle, st: &Shared, url: &str, extra: Extra) -> Result<(), String> {
    let url = valid_link(url)?;
    let id = new_id();
    {
        let mut g = st.inner.lock().unwrap();
        let name = extra.file_name.clone().unwrap_or_else(|| guess_name(&url));
        let mut dir = g.db.settings.default_dir();
        if g.db.settings.sort_by_category && !category_of(&name).is_empty() {
            dir.push(category_of(&name));
        }
        let mut rec = Record::new(id, url, dir, name);
        rec.headers = extra.headers;
        rec.cookie = extra.cookie;
        rec.file_name = extra.file_name;
        g.db.records.push(rec);
        g.db.save(&st.db_path);
    }
    spawn_download(app, st, id);
    Ok(())
}

/// What the "new download" dialog is told about a waiting link. Cookies stay on this side.
#[derive(Serialize, Clone)]
struct PendingView {
    id: String,
    url: String,
    name: String,
    size: Option<u64>,
    /// The site to offer "Don't capture from ..." for.
    site: Option<String>,
    category: &'static str,
    base_dir: String,
    /// Whether the dialog should start in the category's folder.
    sort: bool,
    segments: usize,
    /// Several links sharing one window (from "Grab all links"); empty for a single link.
    batch: Vec<BatchViewItem>,
}

#[derive(Serialize, Clone)]
struct BatchViewItem {
    name: String,
    size: Option<u64>,
}

fn pending_view(p: &Pending, s: &Settings) -> PendingView {
    if !p.batch.is_empty() {
        let known: u64 = p.batch.iter().filter_map(|i| i.size).sum();
        return PendingView {
            id: p.id.to_string(),
            url: p.url.clone(),
            name: format!("{} files", p.batch.len()),
            size: (known > 0).then_some(known),
            site: p.page.as_deref().and_then(host_of).or_else(|| host_of(&p.url)),
            category: "",
            base_dir: s.default_dir().to_string_lossy().into_owned(),
            sort: false,
            segments: s.max_connections,
            batch: p.batch.iter().map(|i| BatchViewItem { name: i.name.clone().unwrap_or_else(|| guess_name(&i.url)), size: i.size }).collect(),
        };
    }
    let name = p.file_name.clone().unwrap_or_else(|| guess_name(&p.url));
    PendingView {
        id: p.id.to_string(),
        url: p.url.clone(),
        category: category_of(&name),
        name,
        size: p.size,
        site: p.page.as_deref().and_then(host_of).or_else(|| host_of(&p.url)),
        base_dir: s.default_dir().to_string_lossy().into_owned(),
        sort: s.sort_by_category,
        segments: s.max_connections,
        batch: Vec::new(),
    }
}

/// A link from the browser or the input box: starts at once, or waits in the "new download" dialog, depending on
/// the setting (`ask` overrides it). Must run inside the Tokio runtime.
pub(crate) fn request_download(app: &AppHandle, st: &Shared, url: &str, extra: Extra, ask: Option<bool>) -> Result<(), String> {
    let url = valid_link(url)?;
    let view = {
        let mut g = st.inner.lock().unwrap();
        if !ask.unwrap_or(g.db.settings.ask_before_download) {
            drop(g);
            return add_url(app, st, &url, extra);
        }
        let p = Pending { id: new_id(), url, headers: extra.headers, cookie: extra.cookie, file_name: extra.file_name, size: extra.size, page: extra.page, batch: Vec::new() };
        let view = pending_view(&p, &g.db.settings);
        g.db.pending.push(p);
        g.db.save(&st.db_path);
        view
    };
    let _ = app.emit("new-download", view);
    Ok(())
}

/// Several links from the browser ("Grab all links"): one dialog for all of them, or, with the dialog off, they just
/// start (in turn, see the queue). Must run inside the Tokio runtime.
pub(crate) fn request_batch(app: &AppHandle, st: &Shared, items: Vec<BatchItem>, extra: Extra, ask: Option<bool>) -> Result<(), String> {
    if items.is_empty() {
        return Err("No links to download.".into());
    }
    let ask = ask.unwrap_or_else(|| st.inner.lock().unwrap().db.settings.ask_before_download);
    if !ask {
        for item in items {
            let one = Extra { headers: extra.headers.clone(), cookie: extra.cookie.clone(), file_name: item.name, size: item.size, page: extra.page.clone() };
            let _ = add_url(app, st, &item.url, one);
        }
        return Ok(());
    }
    let view = {
        let mut g = st.inner.lock().unwrap();
        let p = Pending { id: new_id(), url: items[0].url.clone(), headers: extra.headers, cookie: extra.cookie, file_name: None, size: None, page: extra.page, batch: items };
        let view = pending_view(&p, &g.db.settings);
        g.db.pending.push(p);
        g.db.save(&st.db_path);
        view
    };
    let _ = app.emit("new-download", view);
    Ok(())
}

/// The records a confirmed batch turns into: one per link, all into `dir`, each filed by type if sorting is on.
fn records_from_batch(p: &Pending, dir: &std::path::Path, segments: usize, sort: bool) -> Vec<Record> {
    p.batch
        .iter()
        .map(|item| {
            let name = item.name.as_deref().map(adm_core::sanitize_file_name).unwrap_or_else(|| guess_name(&item.url));
            let mut folder = dir.to_path_buf();
            if sort && !category_of(&name).is_empty() {
                folder.push(category_of(&name));
            }
            let mut rec = Record::new(new_id(), item.url.clone(), folder, name.clone());
            rec.total = item.size;
            rec.headers = p.headers.clone();
            rec.cookie = p.cookie.clone();
            rec.file_name = item.name.as_ref().map(|_| name);
            rec.segments = Some(segments.clamp(1, 32));
            rec
        })
        .collect()
}

/// The record a confirmed dialog turns into.
fn record_from_pending(p: &Pending, name: String, dir: PathBuf, segments: usize) -> Record {
    let mut rec = Record::new(p.id, p.url.clone(), dir, name.clone());
    rec.headers = p.headers.clone();
    rec.cookie = p.cookie.clone();
    rec.file_name = Some(name);
    rec.segments = Some(segments.clamp(1, 32));
    rec
}

// `async` so Tauri runs it inside its Tokio runtime. `ask: false` skips the dialog (several links at once).
#[tauri::command]
async fn add_download(app: AppHandle, st: State<'_, Shared>, url: String, ask: Option<bool>) -> Result<(), String> {
    let shared: Shared = st.inner().clone();
    request_download(&app, &shared, &url, Extra::default(), ask)
}

#[tauri::command]
fn list_pending(st: State<Shared>) -> Vec<PendingView> {
    let g = st.inner.lock().unwrap();
    g.db.pending.iter().map(|p| pending_view(p, &g.db.settings)).collect()
}

#[derive(Serialize)]
struct ProbeView {
    name: String,
    size: Option<u64>,
    resumable: bool,
    category: &'static str,
}

/// Asks the server about a waiting link (name, size, resumable) so the dialog can fill in what the browser
/// did not tell it.
#[tauri::command]
async fn probe_pending(st: State<'_, Shared>, id: String) -> Result<ProbeView, String> {
    let id: u64 = id.parse().map_err(|_| "bad id")?;
    let req = {
        let g = st.inner.lock().unwrap();
        let p = g.db.pending.iter().find(|p| p.id == id).ok_or("This download is no longer waiting.")?;
        if !p.batch.is_empty() {
            return Err("Nothing to check for a group of links.".into());
        }
        let mut r = adm_core::DownloadRequest::new(id, p.url.clone(), g.db.settings.download_dir.clone());
        r.headers = p.headers.clone();
        r.cookie = p.cookie.clone();
        r.file_name = p.file_name.clone();
        r
    };
    let info = st.engine.probe(&req).await.map_err(|e| e.to_string())?;
    {
        let mut g = st.inner.lock().unwrap();
        if let Some(p) = g.db.pending.iter_mut().find(|p| p.id == id) {
            p.size = p.size.or(info.size);
            p.file_name.get_or_insert_with(|| info.file_name.clone());
            g.db.save(&st.db_path);
        }
    }
    Ok(ProbeView { category: category_of(&info.file_name), name: info.file_name, size: info.size, resumable: info.resumable })
}

/// The dialog's "Download": starts the waiting link with the name, folder and connections chosen there.
#[tauri::command]
async fn confirm_pending(app: AppHandle, st: State<'_, Shared>, id: String, name: String, base_dir: String, category: String, segments: usize, remember: Option<String>) -> Result<(), String> {
    let id: u64 = id.parse().map_err(|_| "bad id")?;
    let base = base_dir.trim();
    if base.is_empty() {
        return Err("Choose a folder to save to.".into());
    }
    let mut dir = PathBuf::from(base);
    if CATEGORIES.contains(&category.as_str()) {
        dir.push(&category);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("Can't use that folder: {e}"))?;
    let name = adm_core::sanitize_file_name(name.trim());
    let shared: Shared = st.inner().clone();
    let mut started: Vec<u64> = Vec::new();
    {
        let mut g = shared.inner.lock().unwrap();
        let pos = g.db.pending.iter().position(|p| p.id == id).ok_or("This download is no longer waiting.")?;
        let p = g.db.pending.remove(pos);
        if !p.batch.is_empty() {
            if let Some(chosen) = remember.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
                g.db.settings.remember(std::path::Path::new(chosen));
            }
            let sort = g.db.settings.sort_by_category;
            for rec in records_from_batch(&p, &dir, segments, sort) {
                started.push(rec.id);
                g.db.records.push(rec);
            }
            g.db.save(&shared.db_path);
            drop(g);
            for rid in started {
                spawn_download(&app, &shared, rid);
            }
            return Ok(());
        }
        // Remember the folder the user picked (not a category subfolder of it) for next time.
        if let Some(chosen) = remember.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            g.db.settings.remember(std::path::Path::new(chosen));
        }
        g.db.records.push(record_from_pending(&p, name, dir, segments));
        g.db.save(&shared.db_path);
    }
    started.push(id);
    for rid in started {
        spawn_download(&app, &shared, rid);
    }
    Ok(())
}

#[tauri::command]
fn cancel_pending(st: State<Shared>, id: String) {
    let Ok(id) = id.parse::<u64>() else { return };
    let mut g = st.inner.lock().unwrap();
    g.db.pending.retain(|p| p.id != id);
    g.db.save(&st.db_path);
}

/// "Don't capture from this site": remembers the site (the extension reads the list) and drops the waiting link.
#[tauri::command]
fn block_site(st: State<Shared>, id: String) -> Option<String> {
    let id = id.parse::<u64>().ok()?;
    let mut g = st.inner.lock().unwrap();
    let p = g.db.pending.iter().position(|p| p.id == id).map(|i| g.db.pending.remove(i))?;
    let site = p.page.as_deref().and_then(host_of).or_else(|| host_of(&p.url))?;
    if !g.db.settings.blocked_sites.contains(&site) {
        g.db.settings.blocked_sites.push(site.clone());
    }
    g.db.save(&st.db_path);
    Some(site)
}

/// Free space on the volume that holds `dir` (or the nearest existing parent), in bytes.
#[tauri::command]
fn free_space(dir: String) -> Option<u64> {
    free_space_at(std::path::Path::new(dir.trim()))
}

#[cfg(unix)]
fn free_space_at(dir: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut p = dir;
    while !p.exists() {
        p = p.parent()?;
    }
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `s` a zeroed statvfs the call fills in.
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_space_at(_: &std::path::Path) -> Option<u64> {
    None
}

/// The "Browse..." button: a native folder chooser (`kdialog` or `zenity` on Linux). `Ok(None)` is "cancelled".
#[tauri::command]
async fn browse_folder(start: String) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || pick_folder(&start)).await.map_err(|e| e.to_string())?
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pick_folder(start: &str) -> Result<Option<String>, String> {
    use std::process::Command;
    let start = if std::path::Path::new(start).exists() { start.to_string() } else { std::env::var("HOME").unwrap_or_else(|_| "/".into()) };
    let choosers: [(&str, Vec<String>); 2] = [
        ("kdialog", vec!["--title".into(), "Choose a folder".into(), "--getexistingdirectory".into(), start.clone()]),
        ("zenity", vec!["--file-selection".into(), "--directory".into(), "--title=Choose a folder".into(), format!("--filename={start}/")]),
    ];
    for (program, args) in choosers {
        // A program that is not installed fails to start: try the next. One that ran and exited without a path
        // means the user closed the dialog.
        if let Ok(out) = Command::new(program).args(args).output() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return Ok((out.status.success() && !path.is_empty()).then_some(path));
        }
    }
    Err("No folder chooser found (install kdialog or zenity). Type the folder path instead.".into())
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn pick_folder(_: &str) -> Result<Option<String>, String> {
    Err("The folder chooser is not available here yet. Type the folder path instead.".into())
}

#[tauri::command]
async fn resume(app: AppHandle, st: State<'_, Shared>, id: String) -> Result<(), String> {
    let id = id.parse().map_err(|_| "bad id")?;
    let shared: Shared = st.inner().clone();
    spawn_download(&app, &shared, id);
    Ok(())
}

#[tauri::command]
fn pause(app: AppHandle, st: State<Shared>, id: String) {
    let Ok(id) = id.parse::<u64>() else { return };
    let was_queued = {
        let mut g = st.inner.lock().unwrap();
        if let Some(h) = g.handles.get(&id) {
            h.pause();
            return;
        }
        let before = g.queue.len();
        g.queue.retain(|q| *q != id);
        g.queue.len() != before
    };
    if was_queued {
        st.update(&app, id, true, |r| r.status = Status::Paused);
    }
}

/// Removes a download from the list. A running one is stopped and its partial data deleted; with `delete_file`
/// a finished file is moved to the trash too (the list entry stays if that fails, so nothing is lost silently).
#[tauri::command]
async fn remove(app: AppHandle, st: State<'_, Shared>, id: String, delete_file: Option<bool>) -> Result<(), String> {
    let id: u64 = id.parse().map_err(|_| "bad id")?;
    let handle = st.inner.lock().unwrap().handles.remove(&id);
    if let Some(h) = handle {
        h.pause();
        h.wait().await;
    }
    let trash = {
        let g = st.inner.lock().unwrap();
        g.db.records.iter().find(|r| r.id == id).filter(|r| r.status == Status::Completed && delete_file == Some(true)).and_then(|r| r.path.clone())
    };
    if let Some(path) = trash.filter(|p| p.exists()) {
        let ok = tokio::task::spawn_blocking(move || move_to_trash(&path)).await.unwrap_or(false);
        if !ok {
            return Err("Could not move the file to the trash.".into());
        }
    }
    let dir = {
        let mut g = st.inner.lock().unwrap();
        g.live.remove(&id);
        g.queue.retain(|q| *q != id);
        let dir = g.db.records.iter().find(|r| r.id == id).filter(|r| r.status != Status::Completed).map(|r| r.dir.clone());
        g.db.records.retain(|r| r.id != id);
        g.db.save(&st.db_path);
        dir
    };
    // Unfinished downloads leave a state file and a hidden part file behind; completed ones leave the file.
    if let Some(dir) = dir {
        st.engine.discard(id, &dir);
    }
    let _ = app.emit("removed", id.to_string());
    Ok(())
}

/// Opens a file with the default application.
fn open_with_system(target: PathBuf) {
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener).arg(target).spawn();
}

/// `/home/me/My Files/a b.zip` -> `file:///home/me/My%20Files/a%20b.zip`
fn file_uri(path: &std::path::Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// Shows `path` in the user's file manager, with the file highlighted when it is a file.
///
/// On Linux this uses the `org.freedesktop.FileManager1` D-Bus interface, which every mainstream file manager
/// answers. Asking `xdg-open` to open the folder is not enough: it uses whatever program claims
/// `inode/directory`, which on some systems is a terminal. Falls back to `xdg-open` if no file manager answers.
/// Runs on its own thread because it waits for the file manager's reply.
fn show_in_file_manager(path: PathBuf) {
    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).status();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg("-R").arg(&path).status();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use std::process::{Command, Stdio};
            let method = if path.is_file() { "ShowItems" } else { "ShowFolders" };
            let target = if path.exists() { path.clone() } else { path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.clone()) };
            let shown = Command::new("dbus-send")
                .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "--type=method_call", "/org/freedesktop/FileManager1"])
                .arg(format!("org.freedesktop.FileManager1.{method}"))
                .arg(format!("array:string:{}", file_uri(&target)))
                .arg("string:")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !shown {
                let folder = if target.is_dir() { target } else { target.parent().map(|p| p.to_path_buf()).unwrap_or(target) };
                let _ = Command::new("xdg-open").arg(folder).spawn();
            }
        }
    });
}

/// Moves a file to the trash (recoverable). `false` if that is not possible: nothing is ever deleted for good.
fn move_to_trash(path: &std::path::Path) -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("gio").arg("trash").arg(path).status().is_ok_and(|s| s.success())
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = path;
        false
    }
}

/// Shows a download in the file manager: the finished file highlighted, or the download folder.
#[tauri::command]
fn reveal(st: State<Shared>, id: String) {
    let Ok(id) = id.parse::<u64>() else { return };
    let g = st.inner.lock().unwrap();
    let Some(r) = g.db.records.iter().find(|r| r.id == id) else { return };
    show_in_file_manager(match (&r.status, &r.path) {
        (Status::Completed, Some(p)) => p.clone(),
        _ => r.dir.clone(),
    });
}

/// Opens the finished file with the default application.
#[tauri::command]
fn open_file(st: State<Shared>, id: String) {
    let Ok(id) = id.parse::<u64>() else { return };
    let g = st.inner.lock().unwrap();
    if let Some(path) = g.db.records.iter().find(|r| r.id == id).and_then(|r| r.path.clone()) {
        open_with_system(path);
    }
}

const MAIN: &str = "main";

/// Brings the window up, recreating it if it was closed (closing destroys it to free the webview's memory).
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    } else if let Some(cfg) = app.config().app.windows.first() {
        let _ = tauri::WebviewWindowBuilder::from_config(app, cfg).and_then(|b| b.build());
    }
}

/// Pauses every running download (so each saves its progress), then exits.
fn quit(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let st: Shared = app.state::<Shared>().inner().clone();
        let handles: Vec<DownloadHandle> = st.inner.lock().unwrap().handles.drain().map(|(_, h)| h).collect();
        for h in &handles {
            h.pause();
        }
        for h in handles {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(12), h.wait()).await;
        }
        // Let the event pumps record the Paused status before the process goes away.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        app.exit(0);
    });
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let show = MenuItem::with_id(app, "show", "Show ADM", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit_item])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("ADM")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "show" => show_main(app),
            "quit" => quit(app.clone()),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The browser extension starts ADM through this: answer its one message and leave (no window, no lock).
    if native_host::is_host_launch(&args) {
        return native_host::run();
    }
    match args.first().map(String::as_str) {
        Some("--register-native-host") => {
            for f in native_host::register() {
                println!("registered {}", f.display());
            }
            return;
        }
        Some("--unregister-native-host") => return native_host::unregister(),
        _ => {}
    }
    let background = args.iter().any(|a| a == "--background");
    // The port is the single-instance lock: a second launch hands over to the first and exits.
    let listener = match server::acquire(&args) {
        server::Acquired::Primary(l) => Some(l),
        server::Acquired::AnotherInstance => return,
        server::Acquired::PortTaken => {
            eprintln!("Port {} is used by another program: browser integration is off.", server::port());
            None
        }
    };
    tauri::Builder::default()
        .setup(move |app| {
            // `ADM_DATA_DIR` keeps a separate copy's list and settings apart from the real ones (used by the tests).
            let data = match std::env::var_os("ADM_DATA_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_data_dir().map_err(|e| e.to_string())?,
            };
            std::fs::create_dir_all(&data)?;
            let db_path = data.join("downloads.json");
            let mut db = Db::load(&db_path);
            // Anything that was running when the app last closed is simply paused now.
            for r in &mut db.records {
                if matches!(r.status, Status::Downloading | Status::Queued) {
                    r.status = Status::Paused;
                }
            }
            let engine = Engine::new(EngineConfig::new(data.join("state"))).map_err(|e| e.to_string())?;
            engine.speed_limiter().set_limit(db.settings.speed_limit_kib * 1024);
            app.manage(Arc::new(App {
                engine,
                inner: Mutex::new(Inner { db, handles: HashMap::new(), live: HashMap::new(), queue: VecDeque::new() }),
                db_path,
            }));
            build_tray(app)?;
            // Started by the extension: live in the tray, no window until a download needs one.
            if background {
                if let Some(w) = app.get_webview_window(MAIN) {
                    let _ = w.destroy();
                }
            }
            // Keep the browsers' record of where ADM is up to date (not for the isolated copies the tests run).
            if std::env::var_os("ADM_PORT").is_none() && std::env::var_os("ADM_DATA_DIR").is_none() {
                std::thread::spawn(|| {
                    native_host::register();
                });
            }
            if let Some(listener) = listener {
                let (handle, st): (AppHandle, Shared) = (app.handle().clone(), app.state::<Shared>().inner().clone());
                tauri::async_runtime::spawn(server::serve(listener, handle, st));
            }
            // `adm <link>` adds that link on start-up.
            if let Some(url) = std::env::args().nth(1).filter(|a| a.starts_with("http")) {
                let (handle, st): (AppHandle, Shared) = (app.handle().clone(), app.state::<Shared>().inner().clone());
                tauri::async_runtime::spawn(async move {
                    let _ = add_url(&handle, &st, &url, Extra::default());
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window destroys the webview (its processes use most of the memory); the
            // app and its downloads keep running from the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.destroy();
            }
        })
        .invoke_handler(tauri::generate_handler![list, get_settings, set_settings, add_download, list_pending, probe_pending, confirm_pending, cancel_pending, block_site, free_space, browse_folder, resume, pause, remove, reveal, open_file])
        .build(tauri::generate_context!())
        .expect("failed to start ADM")
        .run(|_app, event| {
            // The last window closing must not end the app; only the tray's Quit (explicit exit code) does.
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}


#[cfg(test)]
mod tests {
    use super::{Pending, Settings, file_uri, free_space_at, host_of, new_id, record_from_pending, records_from_batch};
    use crate::store::BatchItem;
    use crate::store::{Db, category_of};
    use std::path::{Path, PathBuf};

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://User@Example.com:8080/a?b#c").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://127.0.0.1:18095/page.html").as_deref(), Some("127.0.0.1"));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of("https:///x"), None);
    }

    #[test]
    fn categories() {
        assert_eq!(category_of("Movie.MKV"), "Video");
        assert_eq!(category_of("song.flac"), "Music");
        assert_eq!(category_of("paper.pdf"), "Documents");
        assert_eq!(category_of("backup.tar.gz"), "Compressed");
        assert_eq!(category_of("setup.exe"), "Programs");
        assert_eq!(category_of("data.bin"), "");
        assert_eq!(category_of("noext"), "");
    }

    #[test]
    fn a_confirmed_link_becomes_a_download_with_the_dialogs_choices() {
        let p = Pending { id: 7, url: "https://x.io/a.mkv".into(), headers: vec![("Referer".into(), "https://x.io/p".into())], cookie: Some("s=1".into()), file_name: Some("a.mkv".into()), size: Some(5), page: None, batch: vec![] };
        let r = record_from_pending(&p, "My Movie.mkv".into(), PathBuf::from("/d/Video"), 99);
        assert_eq!((r.id, r.name.as_str(), r.file_name.as_deref()), (7, "My Movie.mkv", Some("My Movie.mkv")));
        assert_eq!(r.dir, PathBuf::from("/d/Video"));
        assert_eq!(r.segments, Some(32), "clamped to what the engine allows");
        assert_eq!((r.cookie.as_deref(), r.headers.len()), (Some("s=1"), 1), "the browser's context is kept");
        assert_eq!(record_from_pending(&p, "a".into(), PathBuf::from("/d"), 0).segments, Some(1));
    }

    #[test]
    fn a_list_saved_by_an_older_version_still_loads() {
        // No ask_before_download / sort_by_category / blocked_sites / pending / segments yet.
        let old = r#"{"settings":{"max_connections":4,"speed_limit_kib":100,"download_dir":"/d"},
                      "records":[{"id":1,"url":"http://x/a","dir":"/d","name":"a","total":null,"downloaded":0,"status":"paused","error":null,"path":null}]}"#;
        let db: Db = serde_json::from_str(old).unwrap();
        assert_eq!((db.settings.max_connections, db.settings.speed_limit_kib), (4, 100));
        assert!(db.settings.ask_before_download, "the dialog is on unless turned off");
        assert!(db.settings.remember_folder && db.settings.last_folder.is_none(), "remembering is on, nothing remembered yet");
        assert_eq!(db.settings.max_simultaneous, 3);
        assert!(db.settings.capture_extensions.contains(&"ZIP".to_string()) && db.settings.capture_over_mb == 20, "capture rules default for an old list");
        assert!(!db.settings.sort_by_category && db.settings.blocked_sites.is_empty() && db.pending.is_empty());
        assert_eq!(db.records[0].segments, None);
    }

    #[test]
    fn the_capture_list_typed_in_settings_is_cleaned_up() {
        let typed = [" .Zip", "mp4", "MP4", "a b", "", "tar.gz", "iso"].map(String::from);
        assert_eq!(crate::store::normalize_extensions(typed), ["ZIP", "MP4", "ISO"]);
        assert!(crate::store::normalize_extensions(["x".repeat(11)]).is_empty(), "too long to be an extension");
    }

    #[test]
    fn ids_are_unique_even_when_made_in_the_same_instant() {
        let ids: Vec<u64> = (0..1000).map(|_| new_id()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "strictly increasing");
        assert!(ids.iter().all(|i| *i < (1 << 53)));
    }

    #[test]
    fn a_confirmed_batch_becomes_one_download_per_link() {
        let item = |u: &str, n: Option<&str>, s| BatchItem { url: u.into(), name: n.map(String::from), size: s };
        let p = Pending { id: 1, url: "https://x/a.mp4".into(), headers: vec![("Referer".into(), "https://x/p".into())], cookie: Some("s=1".into()), file_name: None, size: None, page: None,
            batch: vec![item("https://x/a.mp4", Some("Clip One.mp4"), Some(10)), item("https://x/b.zip?t=1", None, None), item("https://x/c.pdf", Some("../../evil.pdf"), None)] };
        let flat = records_from_batch(&p, Path::new("/d"), 99, false);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Clip One.mp4", "b.zip", "evil.pdf"], "page names kept, others from the link, unsafe parts dropped");
        assert!(flat.iter().all(|r| r.dir == Path::new("/d") && r.segments == Some(32) && r.cookie.as_deref() == Some("s=1") && r.headers.len() == 1));
        let mut ids: Vec<u64> = flat.iter().map(|r| r.id).collect(); ids.dedup();
        assert_eq!(ids.len(), 3, "each gets its own id");
        assert_eq!((flat[0].total, flat[1].total), (Some(10), None));
        let sorted = records_from_batch(&p, Path::new("/d"), 8, true);
        assert_eq!(sorted.iter().map(|r| r.dir.to_str().unwrap()).collect::<Vec<_>>(), ["/d/Video", "/d/Compressed", "/d/Documents"], "filed by type when sorting is on");
    }

    #[test]
    fn the_last_chosen_folder_is_remembered_and_the_default_can_win_again() {
        let base = std::env::temp_dir().join("adm-remember-test");
        let picked = base.join("Picked");
        std::fs::create_dir_all(&picked).unwrap();
        let mut s = Settings { download_dir: base.clone(), ..Settings::default() };
        assert_eq!(s.default_dir(), base, "nothing remembered yet: the download folder");
        s.remember(&picked);
        assert_eq!(s.default_dir(), picked, "the folder picked last time");
        s.remember(&base.join("does-not-exist"));
        assert_eq!(s.default_dir(), picked, "a folder that does not exist is not remembered");
        std::fs::remove_dir_all(&picked).unwrap();
        assert_eq!(s.default_dir(), base, "a remembered folder that is gone falls back to the download folder");
        std::fs::create_dir_all(&picked).unwrap();
        s.remember_folder = false;
        s.remember(&base);
        assert_eq!(s.default_dir(), base, "switched off: always the download folder");
        s.remember_folder = true;
        assert_eq!(s.default_dir(), picked, "and switching it back on finds the old choice");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn free_space_of_a_folder_that_does_not_exist_yet() {
        let tmp = std::env::temp_dir();
        let here = free_space_at(&tmp).expect("temp dir has a volume");
        assert!(here > 0);
        assert_eq!(free_space_at(&tmp.join("adm-not-created-yet").join("deeper")), Some(here), "uses the nearest existing parent");
    }

    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(file_uri(Path::new("/home/me/Downloads/a.zip")), "file:///home/me/Downloads/a.zip");
        assert_eq!(file_uri(Path::new("/home/me/My Files/a b (1).zip")), "file:///home/me/My%20Files/a%20b%20%281%29.zip");
        assert_eq!(file_uri(Path::new("/d/a,b.mkv")), "file:///d/a%2Cb.mkv", "a comma would split the D-Bus array");
        assert_eq!(file_uri(Path::new("/d/résumé.pdf")), "file:///d/r%C3%A9sum%C3%A9.pdf");
    }
}
