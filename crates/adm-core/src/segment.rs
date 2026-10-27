//! One byte range of the file, downloaded by one task over one connection.
//!
//! The segment's `length` can shrink while it runs: the coordinator hands the back half of the remaining
//! range to a new segment (a "split") to keep every connection busy. So a segment never writes a byte
//! it has not first *reserved* under its lock, and a split never cuts into reserved bytes. That is the
//! whole concurrency contract; there is no other lock around the file.

use crate::engine::{ConnectErr, Shared};
use crate::error::DownloadError;
use crate::store::SegStatus;
use crate::util::write_all_at;
use futures_util::StreamExt;
use reqwest::Response;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// A segment of unknown size (no Content-Length) reads until the server closes the stream.
pub(crate) const UNKNOWN_LEN: u64 = u64::MAX;

const WRITE_BUF: usize = 256 * 1024;
/// Bytes sit in memory at most this long, so a slow link still shows progress and loses little on a crash.
const FLUSH_EVERY: Duration = Duration::from_millis(250);

pub(crate) struct Segment {
    pub id: u64,
    pub offset: u64,
    pub st: Mutex<SegState>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SegState {
    pub length: u64,
    /// Bytes claimed for writing. Never exceeds `length`, so a split can trust `length - reserved`.
    pub reserved: u64,
    /// Bytes whose write has returned.
    pub downloaded: u64,
    /// Bytes known to be on stable storage.
    pub synced: u64,
    pub status: SegStatus,
}

impl Segment {
    pub fn new(id: u64, offset: u64, length: u64, progress: u64, status: SegStatus) -> Arc<Self> {
        Arc::new(Self {
            id,
            offset,
            st: Mutex::new(SegState { length, reserved: progress, downloaded: progress, synced: progress, status }),
        })
    }

    pub fn state(&self) -> SegState {
        *self.st.lock().unwrap()
    }
}

pub(crate) enum SegMsg {
    Finished(u64),
    Failed(u64, DownloadError),
    /// Ended because the download was paused or aborted.
    Stopped,
}

enum End {
    Finished,
    Stopped,
    Failed(DownloadError),
}

pub(crate) async fn run(sh: Arc<Shared>, seg: Arc<Segment>, first: Option<Response>, tx: UnboundedSender<SegMsg>) {
    let msg = match drive(&sh, &seg, first).await {
        End::Finished => SegMsg::Finished(seg.id),
        End::Stopped => SegMsg::Stopped,
        End::Failed(e) => SegMsg::Failed(seg.id, e),
    };
    let _ = tx.send(msg);
}

pub(crate) fn backoff(retries: u32) -> Duration {
    Duration::from_secs((1u64 << retries.min(5)).min(30))
}

/// `false` if the download was stopped while waiting.
async fn sleep_unless_stopped(stop: &CancellationToken, d: Duration) -> bool {
    if d.is_zero() {
        return !stop.is_cancelled();
    }
    tokio::select! {
        _ = stop.cancelled() => false,
        _ = tokio::time::sleep(d) => true,
    }
}

async fn drive(sh: &Arc<Shared>, seg: &Arc<Segment>, first: Option<Response>) -> End {
    let ranged = sh.ranged.load(Ordering::Relaxed);
    let max_retries = sh.cfg.max_retries;
    let mut retries = 0u32;
    let mut response = first;
    loop {
        if sh.stop.is_cancelled() {
            return End::Stopped;
        }
        let (start, end_incl, before) = {
            let st = seg.st.lock().unwrap();
            if st.reserved >= st.length {
                return End::Finished;
            }
            let end = (st.length != UNKNOWN_LEN).then(|| seg.offset + st.length - 1);
            (seg.offset + st.reserved, end, st.downloaded)
        };
        let resp = match response.take() {
            Some(r) => r,
            None => match sh.connect(start, end_incl, ranged).await {
                Ok(r) => r,
                Err(ConnectErr::Retry(delay)) => {
                    // A zero delay means "the link was refreshed, ask again", which is not a failed attempt.
                    if delay != Some(Duration::ZERO) {
                        retries += 1;
                        if retries > max_retries {
                            return End::Failed(DownloadError::Network);
                        }
                    }
                    if !sleep_unless_stopped(&sh.stop, delay.unwrap_or_else(|| backoff(retries))).await {
                        return End::Stopped;
                    }
                    continue;
                }
                Err(ConnectErr::Fatal(e)) => return End::Failed(e),
                Err(ConnectErr::Status(code)) => return End::Failed(DownloadError::Http(code)),
            },
        };

        match copy(sh, seg, resp).await {
            Copy::Done => return End::Finished,
            Copy::Cancelled => return End::Stopped,
            Copy::Disk => return End::Failed(DownloadError::Disk),
            Copy::Retry => {
                let progressed = seg.st.lock().unwrap().downloaded > before;
                if ranged && progressed {
                    // Data arrived before the connection broke (e.g. a stalled link): not a failed attempt.
                    retries = 0;
                } else {
                    retries += 1;
                    if retries > max_retries {
                        return End::Failed(DownloadError::Network);
                    }
                }
                if !ranged {
                    restart_from_zero(sh, seg);
                }
                if !sleep_unless_stopped(&sh.stop, backoff(retries)).await {
                    return End::Stopped;
                }
            }
        }
    }
}

/// Without range support a broken connection can only be redone from the first byte.
fn restart_from_zero(sh: &Shared, seg: &Segment) {
    let mut st = seg.st.lock().unwrap();
    sh.session_bytes.fetch_sub(st.downloaded, Ordering::Relaxed);
    sh.total_bytes.fetch_sub(st.downloaded, Ordering::Relaxed);
    st.reserved = 0;
    st.downloaded = 0;
    st.synced = 0;
}

/// Why the response body stopped delivering data.
enum Ended {
    Eof,
    Broken,
}

enum Copy {
    Done,
    Cancelled,
    Disk,
    Retry,
}

async fn copy(sh: &Arc<Shared>, seg: &Arc<Segment>, resp: Response) -> Copy {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(WRITE_BUF);
    let mut last_flush = Instant::now();
    loop {
        let next = tokio::select! {
            biased;
            _ = sh.stop.cancelled() => {
                // Keep what already arrived, so a resume does not fetch it again.
                let _ = flush(sh, seg, &mut buf).await;
                return Copy::Cancelled;
            }
            r = tokio::time::timeout(sh.cfg.read_timeout, stream.next()) => r,
        };
        let ended = match next {
            Ok(Some(Ok(bytes))) => {
                buf.extend_from_slice(&bytes);
                if buf.len() < WRITE_BUF && last_flush.elapsed() < FLUSH_EVERY {
                    continue;
                }
                None
            }
            Ok(None) => Some(Ended::Eof),
            Ok(Some(Err(_))) | Err(_) => Some(Ended::Broken),
        };

        let (wrote, complete) = match flush(sh, seg, &mut buf).await {
            Ok(r) => r,
            Err(()) => return Copy::Disk,
        };
        last_flush = Instant::now();
        // Throttle before any return, or a segment's last flush (the whole segment, for a small file) escapes it.
        let wait = sh.limiter.reserve(wrote);
        if !wait.is_zero() && !sleep_unless_stopped(&sh.stop, wait).await {
            return Copy::Cancelled;
        }
        if complete {
            return Copy::Done;
        }
        if let Some(end) = ended {
            // EOF is the normal end of a stream of unknown length, and a premature close for a sized one.
            let unsized_stream = seg.state().length == UNKNOWN_LEN;
            return if unsized_stream && matches!(end, Ended::Eof) { Copy::Done } else { Copy::Retry };
        }
    }
}

/// Writes the buffer, truncated to whatever the segment may still claim. Returns the bytes written and
/// whether the segment is now complete. `Err` is a disk error.
async fn flush(sh: &Arc<Shared>, seg: &Arc<Segment>, buf: &mut Vec<u8>) -> Result<(u64, bool), ()> {
    let (m, pos, complete) = {
        let mut st = seg.st.lock().unwrap();
        let allowed = st.length.saturating_sub(st.reserved);
        let m = (buf.len() as u64).min(allowed);
        let pos = seg.offset + st.reserved;
        st.reserved += m;
        (m, pos, st.reserved >= st.length)
    };
    if m > 0 {
        let file = sh.file();
        let owned = std::mem::take(buf);
        let (owned, res) = tokio::task::spawn_blocking(move || {
            let r = write_all_at(&file, &owned[..m as usize], pos);
            (owned, r)
        })
        .await
        .map_err(|_| ())?;
        *buf = owned;
        if let Err(e) = res {
            tracing::error!("write failed: {e}");
            return Err(());
        }
        seg.st.lock().unwrap().downloaded += m;
        sh.session_bytes.fetch_add(m, Ordering::Relaxed);
        sh.total_bytes.fetch_add(m, Ordering::Relaxed);
        sh.unsynced.fetch_add(m, Ordering::Relaxed);
    }
    buf.clear();
    Ok((m, complete))
}
