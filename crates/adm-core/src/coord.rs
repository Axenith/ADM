//! The coordinator: one task per download that owns every decision, so none of it needs locks.
//!
//! Segment tasks only move bytes and report back. This task starts them, splits the busiest one when a
//! connection frees up, restarts failed ones, fsyncs and saves progress, and publishes the finished file.

use crate::engine::{ConnectErr, Shared};
use crate::error::DownloadError;
use crate::model::*;
use crate::progress::{SpeedTracker, eta_secs};
use crate::segment::{self, SegMsg, Segment, UNKNOWN_LEN, backoff};
use crate::store::{self, SavedSegment, SavedState, SegStatus, STATE_VERSION};
use crate::util::*;
use reqwest::header::{ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_RANGE, CONTENT_TYPE, ETAG, LAST_MODIFIED};
use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// A range is only split when at least this much of it is still to come (so each half is >= 128 KiB).
const MIN_SPLIT: u64 = 256 * 1024;
/// Data is forced to disk after this many bytes, or this long, whichever comes first. Only synced
/// progress is saved, so this is how far a resume after a crash can fall behind.
const SYNC_BYTES: u64 = 64 << 20;
const SYNC_INTERVAL: Duration = Duration::from_secs(30);
/// How many times a segment may fail (each failure already exhausted its own retries) before the
/// download gives up.
const MAX_ROUNDS: u32 = 2;

pub(crate) async fn run(sh: Arc<Shared>, user: CancellationToken) -> Outcome {
    let outcome = match Coord::start(sh.clone(), user).await {
        Ok(c) => c.main_loop().await,
        Err(o) => o,
    };
    sh.emit(match &outcome {
        Outcome::Completed(f) => Event::Completed(f.clone()),
        Outcome::Paused => Event::Paused,
        Outcome::Failed(e) => Event::Failed(*e),
    });
    outcome
}

struct Meta {
    file_name: String,
    total: Option<u64>,
    content_type: Option<String>,
    completed: bool,
}

struct Coord {
    sh: Arc<Shared>,
    user: CancellationToken,
    segs: BTreeMap<u64, Arc<Segment>>,
    tx: UnboundedSender<SegMsg>,
    rx: UnboundedReceiver<SegMsg>,
    tasks: JoinSet<()>,
    meta: Meta,
    validator: Option<String>,
    rounds: HashMap<u64, u32>,
    last_error: Option<DownloadError>,
    speed: SpeedTracker,
    last_sync: Instant,
    last_split_check: Instant,
    next_id: u64,
}

impl Coord {
    fn new(sh: Arc<Shared>, user: CancellationToken, meta: Meta, validator: Option<String>) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            sh,
            user,
            segs: BTreeMap::new(),
            tx,
            rx,
            tasks: JoinSet::new(),
            meta,
            validator,
            rounds: HashMap::new(),
            last_error: None,
            speed: SpeedTracker::new(0),
            last_sync: Instant::now(),
            last_split_check: Instant::now(),
            next_id: 0,
        }
    }

    async fn start(sh: Arc<Shared>, user: CancellationToken) -> Result<Coord, Outcome> {
        match store::load(&sh.cfg.state_dir, sh.req.id) {
            Some(saved) => Self::resume(sh, user, saved).await,
            None => Self::fresh(sh, user).await,
        }
    }

    // ---- starting ---------------------------------------------------------------------------------

    async fn fresh(sh: Arc<Shared>, user: CancellationToken) -> Result<Coord, Outcome> {
        let failed = Outcome::Failed;
        tokio::fs::create_dir_all(&sh.req.dest_dir).await.map_err(|_| failed(DownloadError::Disk))?;

        // The first request doubles as the probe: `Range: bytes=0-` tells us the size, whether ranges work,
        // and the file's name, and its body is not wasted: it becomes segment 0's connection.
        let mut tries = 0u32;
        let mut probe_ranged = true;
        let resp = loop {
            let attempt = tokio::select! {
                _ = user.cancelled() => return Err(Outcome::Paused),
                r = sh.connect(0, None, probe_ranged) => r,
            };
            match attempt {
                Ok(r) => break r,
                Err(ConnectErr::Retry(d)) => {
                    if d != Some(Duration::ZERO) {
                        tries += 1;
                        if tries > sh.cfg.max_retries {
                            return Err(failed(DownloadError::Network));
                        }
                    }
                    tokio::select! {
                        _ = user.cancelled() => return Err(Outcome::Paused),
                        _ = tokio::time::sleep(d.unwrap_or_else(|| backoff(tries))) => {}
                    }
                }
                Err(ConnectErr::Fatal(e)) => return Err(failed(e)),
                // 416 to `bytes=0-` means an empty file: ask again without a range.
                Err(ConnectErr::Status(416)) if probe_ranged => probe_ranged = false,
                Err(ConnectErr::Status(code)) => return Err(failed(DownloadError::Http(code))),
            }
        };

        let status = resp.status().as_u16();
        let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let (total, mut ranged) = if status == 206 {
            match header(CONTENT_RANGE).as_deref().and_then(parse_content_range) {
                Some((0, _, Some(total))) => (Some(total), true),
                _ => (None, false), // ranges, but no usable size: cannot split
            }
        } else {
            (resp.content_length(), false)
        };
        // Some servers answer an open-ended `bytes=0-` with 200 yet honour bounded ranges. Ask for the last
        // byte to find out; the probe's own stream (the whole file) stays usable as segment 0 either way.
        let accepts_none = header(ACCEPT_RANGES).is_some_and(|v| v.trim().eq_ignore_ascii_case("none"));
        if status == 200 && !accepts_none {
            if let Some(t) = total.filter(|t| *t > 0) {
                ranged = tokio::select! {
                    _ = user.cancelled() => return Err(Outcome::Paused),
                    r = sh.connect(t - 1, Some(t - 1), true) => r.is_ok(),
                };
            }
        }
        // Last-Modified rather than ETag first: mirrors of one file have their own ETags but share a date.
        let validator = header(LAST_MODIFIED).or_else(|| header(ETAG).filter(|e| !e.starts_with("W/")));
        let file_name = derive_file_name(
            sh.req.file_name.as_deref(),
            header(CONTENT_DISPOSITION).as_deref(),
            &sh.req.url,
            resp.url().as_str(),
        );
        let meta = Meta { file_name, total, content_type: header(CONTENT_TYPE), completed: false };

        // Sized up front so segments can write anywhere; the OS keeps it sparse until bytes arrive.
        let part = part_path(&sh.req.dest_dir, sh.req.id);
        let file = tokio::task::spawn_blocking(move || {
            let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&part)?;
            if let Some(t) = total {
                f.set_len(t)?;
            }
            Ok::<_, std::io::Error>(f)
        })
        .await
        .map_err(|_| failed(DownloadError::Internal))?
        .map_err(|e| {
            tracing::error!("cannot create part file: {e}");
            failed(DownloadError::Disk)
        })?;

        sh.ranged.store(ranged, Ordering::Relaxed);
        let _ = sh.validator.set(validator.clone());
        let _ = sh.file.set(Arc::new(file));

        let mut c = Coord::new(sh.clone(), user, meta, validator);
        sh.emit(Event::Init(c.init_info(resp.url().to_string(), ranged)));
        let seg0 = Segment::new(c.take_id(), 0, total.unwrap_or(UNKNOWN_LEN), 0, SegStatus::Pending);
        c.spawn(seg0, Some(resp));
        c.split_to_max();
        c.save().await;
        Ok(c)
    }

    async fn resume(sh: Arc<Shared>, user: CancellationToken, saved: SavedState) -> Result<Coord, Outcome> {
        let part = part_path(&sh.req.dest_dir, sh.req.id);
        if !saved.completed && (!saved.resumable || saved.total_size.is_none()) {
            // Nothing can be continued (no size, or the server ignores ranges): start over.
            tracing::info!("download {} cannot resume, starting over", sh.req.id);
            let _ = std::fs::remove_file(&part);
            store::remove(&sh.cfg.state_dir, sh.req.id);
            return Self::fresh(sh, user).await;
        }
        // A part file that is gone or the wrong size is a disk problem, never a reason to silently start over.
        let total = saved.total_size;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&part)
            .ok()
            .filter(|f| match (total, f.metadata()) {
                (Some(t), Ok(m)) => m.len() == t,
                (None, Ok(_)) => true,
                _ => false,
            })
            .ok_or(Outcome::Failed(DownloadError::Disk))?;

        sh.ranged.store(saved.resumable, Ordering::Relaxed);
        let _ = sh.validator.set(saved.validator.clone());
        let _ = sh.file.set(Arc::new(file));

        let meta = Meta {
            file_name: saved.file_name,
            total,
            content_type: saved.content_type,
            completed: saved.completed,
        };
        let mut c = Coord::new(sh.clone(), user, meta, saved.validator);
        let mut restored = 0;
        for s in saved.segments {
            let progress = s.synced.min(s.length);
            let status = if progress >= s.length { SegStatus::Finished } else { SegStatus::Pending };
            restored += progress;
            c.next_id = c.next_id.max(s.id + 1);
            c.segs.insert(s.id, Segment::new(s.id, s.offset, s.length, progress, status));
        }
        sh.total_bytes.store(restored, Ordering::Relaxed);
        sh.emit(Event::Init(c.init_info(sh.req.url.clone(), saved.resumable)));
        c.fill_slots();
        Ok(c)
    }

    fn init_info(&self, final_url: String, resumable: bool) -> InitInfo {
        InitInfo {
            id: self.sh.req.id,
            final_url,
            file_name: self.meta.file_name.clone(),
            total_size: self.meta.total,
            content_type: self.meta.content_type.clone(),
            resumable,
        }
    }

    // ---- segments ---------------------------------------------------------------------------------

    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    fn running(&self) -> usize {
        self.segs.values().filter(|s| s.state().status == SegStatus::Running).count()
    }

    fn spawn(&mut self, seg: Arc<Segment>, first: Option<reqwest::Response>) {
        seg.st.lock().unwrap().status = SegStatus::Running;
        self.segs.insert(seg.id, seg.clone());
        self.tasks.spawn(segment::run(self.sh.clone(), seg, first, self.tx.clone()));
    }

    fn set_status(&self, id: u64, status: SegStatus) {
        if let Some(s) = self.segs.get(&id) {
            s.st.lock().unwrap().status = status;
        }
    }

    /// Starts waiting segments (never started, or paused) while there are free connections, then failed
    /// ones that still have a round left.
    fn fill_slots(&mut self) {
        let mut free = self.sh.max_segments.saturating_sub(self.running());
        let mut order: Vec<_> = self.segs.values().cloned().collect();
        order.sort_by_key(|s| s.offset);
        for retry_failed in [false, true] {
            for s in &order {
                if free == 0 {
                    return;
                }
                let eligible = match (s.state().status, retry_failed) {
                    (SegStatus::Pending, false) => true,
                    (SegStatus::Failed, true) => self.rounds.get(&s.id).copied().unwrap_or(0) < MAX_ROUNDS,
                    _ => false,
                };
                if eligible {
                    self.spawn(s.clone(), None);
                    free -= 1;
                }
            }
        }
    }

    /// Keeps every connection busy: while one is free, halve the running segment with the most left.
    fn split_to_max(&mut self) {
        if !self.sh.ranged.load(Ordering::Relaxed) || self.meta.total.is_none() {
            return;
        }
        while self.running() < self.sh.max_segments && self.split_largest() {}
    }

    fn split_largest(&mut self) -> bool {
        let Some(parent) = self
            .segs
            .values()
            .filter(|s| s.state().status == SegStatus::Running)
            .max_by_key(|s| {
                let st = s.state();
                st.length - st.reserved
            })
            .cloned()
        else {
            return false;
        };
        // Cut under the segment's lock: bytes it has reserved are never taken away from it.
        let (offset, len) = {
            let mut st = parent.st.lock().unwrap();
            let rem = st.length - st.reserved;
            if rem < MIN_SPLIT {
                return false;
            }
            let len = rem / 2;
            st.length -= len;
            (parent.offset + st.length, len)
        };
        let child = Segment::new(self.take_id(), offset, len, 0, SegStatus::Pending);
        tracing::debug!("split segment {} -> new segment {} at {offset} (+{len})", parent.id, child.id);
        self.spawn(child, None);
        true
    }

    // ---- main loop --------------------------------------------------------------------------------

    async fn main_loop(mut self) -> Outcome {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        if let Some(o) = self.check_done().await {
            return o;
        }
        loop {
            tokio::select! {
                biased;
                _ = self.user.cancelled() => return self.pause().await,
                Some(msg) = self.rx.recv() => {
                    if let Some(o) = self.on_msg(msg).await {
                        return o;
                    }
                }
                _ = tick.tick() => self.on_tick().await,
            }
            if let Some(o) = self.check_done().await {
                return o;
            }
        }
    }

    async fn on_msg(&mut self, msg: SegMsg) -> Option<Outcome> {
        match msg {
            SegMsg::Finished(id) => {
                self.set_status(id, SegStatus::Finished);
                self.fill_slots();
                self.split_to_max();
                self.save().await;
            }
            SegMsg::Failed(id, e) => {
                tracing::warn!("segment {id} failed: {e}");
                self.set_status(id, SegStatus::Failed);
                self.last_error = Some(e);
                let round = self.rounds.entry(id).or_default();
                *round += 1;
                match e {
                    DownloadError::Disk => return Some(self.abort(e).await),
                    // More attempts cannot change these.
                    DownloadError::Tls
                    | DownloadError::LinkExpired
                    | DownloadError::ResumeNotSupported
                    | DownloadError::InvalidResponse
                    | DownloadError::Http(_) => *round = MAX_ROUNDS,
                    _ => {}
                }
                self.fill_slots();
            }
            SegMsg::Stopped => {}
        }
        None
    }

    async fn check_done(&mut self) -> Option<Outcome> {
        if self.segs.is_empty() {
            return None;
        }
        if self.segs.values().all(|s| s.state().status == SegStatus::Finished) {
            return Some(self.finish().await);
        }
        // A failed segment with no round left can never complete, so the download cannot either.
        let dead = self
            .segs
            .values()
            .any(|s| s.state().status == SegStatus::Failed && self.rounds.get(&s.id).copied().unwrap_or(0) >= MAX_ROUNDS);
        if dead {
            let e = self.last_error.unwrap_or(DownloadError::Network);
            return Some(self.abort(e).await);
        }
        None
    }

    async fn on_tick(&mut self) {
        let downloaded = self.downloaded();
        let speed = self.speed.sample(self.sh.session_bytes.load(Ordering::Relaxed));
        let total = self.meta.total;
        let mut segments: Vec<_> = self
            .segs
            .values()
            .map(|s| {
                let st = s.state();
                SegmentProgress { offset: s.offset, length: st.length, downloaded: st.downloaded.min(st.length) }
            })
            .collect();
        segments.sort_by_key(|s| s.offset);
        self.sh.emit(Event::Progress(ProgressInfo {
            id: self.sh.req.id,
            downloaded,
            total,
            speed,
            eta_secs: total.and_then(|t| eta_secs(t.saturating_sub(downloaded), speed)),
            segments,
        }));

        // A free connection is used right away on events; this catches the cases that raise no event.
        if self.last_split_check.elapsed() >= Duration::from_secs(1) {
            self.last_split_check = Instant::now();
            self.fill_slots();
            self.split_to_max();
        }
        let unsynced = self.sh.unsynced.load(Ordering::Relaxed);
        if unsynced >= SYNC_BYTES || (unsynced > 0 && self.last_sync.elapsed() >= SYNC_INTERVAL) {
            self.sync().await;
            self.save().await;
        }
    }

    fn downloaded(&self) -> u64 {
        self.segs.values().map(|s| s.state()).map(|st| st.downloaded.min(st.length)).sum()
    }

    // ---- persistence ------------------------------------------------------------------------------

    /// Forces the part file to disk, then marks the bytes that were written *before* the flush as synced.
    async fn sync(&mut self) {
        self.last_sync = Instant::now();
        let before: Vec<_> = self.segs.values().map(|s| (s.clone(), s.state().downloaded)).collect();
        let pending = self.sh.unsynced.swap(0, Ordering::Relaxed);
        let file = self.sh.file();
        match tokio::task::spawn_blocking(move || file.sync_data()).await {
            Ok(Ok(())) => {
                for (s, n) in before {
                    let mut st = s.st.lock().unwrap();
                    st.synced = st.synced.max(n.min(st.length));
                }
            }
            _ => {
                tracing::error!("fsync failed");
                self.sh.unsynced.fetch_add(pending, Ordering::Relaxed);
            }
        }
    }

    async fn save(&self) {
        let segments = self
            .segs
            .values()
            .map(|s| {
                let st = s.state();
                SavedSegment {
                    id: s.id,
                    offset: s.offset,
                    length: st.length,
                    synced: st.synced,
                    // Complete only once all of it is known to be on disk.
                    status: if st.synced >= st.length { SegStatus::Finished } else { SegStatus::Pending },
                }
            })
            .collect();
        let state = SavedState {
            version: STATE_VERSION,
            id: self.sh.req.id,
            url: self.sh.req.url.clone(),
            file_name: self.meta.file_name.clone(),
            total_size: self.meta.total,
            content_type: self.meta.content_type.clone(),
            validator: self.validator.clone(),
            resumable: self.sh.ranged.load(Ordering::Relaxed),
            completed: self.meta.completed,
            segments,
        };
        let dir = self.sh.cfg.state_dir.clone();
        match tokio::task::spawn_blocking(move || store::save(&dir, &state)).await {
            Ok(Ok(())) => {}
            other => tracing::error!("cannot save state: {other:?}"),
        }
    }

    // ---- ending -----------------------------------------------------------------------------------

    /// Waits for the segment tasks to flush and exit.
    async fn drain(&mut self) {
        let all = async { while self.tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(Duration::from_secs(10), all).await.is_err() {
            self.tasks.abort_all();
            while self.tasks.join_next().await.is_some() {}
        }
    }

    /// Everything received so far is on disk and recorded; the next start continues from it.
    async fn settle(&mut self) {
        self.sh.stop.cancel();
        self.drain().await;
        self.sync().await;
        self.save().await;
    }

    async fn pause(mut self) -> Outcome {
        self.settle().await;
        Outcome::Paused
    }

    async fn abort(&mut self, e: DownloadError) -> Outcome {
        self.settle().await;
        // Several segments refused after a good start: the session behind the link ended.
        let e = if matches!(e, DownloadError::InvalidResponse | DownloadError::Http(_))
            && self.sh.total_bytes.load(Ordering::Relaxed) > 0
            && self.segs.len() > 2
        {
            DownloadError::SessionExpired
        } else {
            e
        };
        Outcome::Failed(e)
    }

    /// Whether the segments really make up the file: they must cover `0..total` exactly, each fully
    /// downloaded.
    fn pieces_make_up_file(&self) -> bool {
        let Some(total) = self.meta.total else { return false };
        let mut segs: Vec<_> = self.segs.values().map(|s| (s.offset, s.state())).collect();
        segs.sort_by_key(|(o, _)| *o);
        let mut next = 0;
        for (offset, st) in segs {
            if offset != next || st.downloaded < st.length {
                return false;
            }
            next += st.length;
        }
        next == total
    }

    async fn finish(&mut self) -> Outcome {
        self.drain().await;
        let file = self.sh.file();
        if self.meta.total.is_none() {
            // A stream of unknown size: what arrived is the file.
            let n = self.downloaded_unsized();
            for s in self.segs.values() {
                s.st.lock().unwrap().length = n;
            }
            self.meta.total = Some(n);
            let f = file.clone();
            if !matches!(tokio::task::spawn_blocking(move || f.set_len(n)).await, Ok(Ok(()))) {
                return self.fail(DownloadError::Disk).await;
            }
        }
        if !self.pieces_make_up_file() {
            tracing::error!("segments do not make up the file, not publishing it");
            return self.fail(DownloadError::Internal).await;
        }
        if !matches!(tokio::task::spawn_blocking(move || file.sync_all()).await, Ok(Ok(()))) {
            return self.fail(DownloadError::Disk).await;
        }
        for s in self.segs.values() {
            let mut st = s.st.lock().unwrap();
            st.synced = st.downloaded;
        }
        // Saved as complete before publishing: if the move fails, the next start only has to retry the move.
        self.meta.completed = true;
        self.save().await;

        let size = self.meta.total.unwrap_or(0);
        let part = part_path(&self.sh.req.dest_dir, self.sh.req.id);
        let (dir, name) = (self.sh.req.dest_dir.clone(), self.meta.file_name.clone());
        match tokio::task::spawn_blocking(move || publish(&part, &dir, &name)).await {
            Ok(Ok(path)) => {
                store::remove(&self.sh.cfg.state_dir, self.sh.req.id);
                Outcome::Completed(FinalInfo { id: self.sh.req.id, size, path })
            }
            other => {
                tracing::error!("cannot publish the finished file: {other:?}");
                Outcome::Failed(DownloadError::OutputWrite)
            }
        }
    }

    fn downloaded_unsized(&self) -> u64 {
        self.segs.values().map(|s| s.state().downloaded).sum()
    }

    async fn fail(&mut self, e: DownloadError) -> Outcome {
        self.save().await;
        Outcome::Failed(e)
    }
}
