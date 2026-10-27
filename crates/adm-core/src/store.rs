//! Progress persistence. One small JSON file per download, replaced atomically (write temp, fsync,
//! rename) so a crash leaves either the old state or the new one, never half of each.
//!
//! Only progress lives here. The part file's location and the request headers (which hold cookies and
//! credentials) are supplied again by the caller on resume.

use crate::model::DownloadId;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegStatus {
    Pending,
    Running,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSegment {
    pub id: u64,
    pub offset: u64,
    pub length: u64,
    /// Bytes known to be on disk (fsynced), counted from `offset`.
    pub synced: u64,
    pub status: SegStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedState {
    pub version: u32,
    pub id: DownloadId,
    pub url: String,
    pub file_name: String,
    pub total_size: Option<u64>,
    pub content_type: Option<String>,
    /// `Last-Modified` / strong `ETag` of the first response; sent as `If-Range` so a changed file is detected.
    pub validator: Option<String>,
    pub resumable: bool,
    pub completed: bool,
    pub segments: Vec<SavedSegment>,
}

pub fn state_path(dir: &Path, id: DownloadId) -> PathBuf {
    dir.join(format!("{id}.state"))
}

pub fn save(dir: &Path, state: &SavedState) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let final_path = state_path(dir, state.id);
    let tmp = final_path.with_extension("state.tmp");
    let json = serde_json::to_vec(state).map_err(io::Error::other)?;
    {
        let mut f = File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &final_path)
}

/// `Ok(None)` when there is no saved state or it cannot be understood (the download then starts fresh).
pub fn load(dir: &Path, id: DownloadId) -> Option<SavedState> {
    let bytes = fs::read(state_path(dir, id)).ok()?;
    let state: SavedState = serde_json::from_slice(&bytes).ok()?;
    (state.version == STATE_VERSION && state.id == id).then_some(state)
}

pub fn remove(dir: &Path, id: DownloadId) {
    let _ = fs::remove_file(state_path(dir, id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let st = SavedState {
            version: STATE_VERSION,
            id: 7,
            url: "http://x/y".into(),
            file_name: "y".into(),
            total_size: Some(10),
            content_type: None,
            validator: None,
            resumable: true,
            completed: false,
            segments: vec![SavedSegment { id: 1, offset: 0, length: 10, synced: 4, status: SegStatus::Pending }],
        };
        save(dir.path(), &st).unwrap();
        assert_eq!(load(dir.path(), 7).unwrap().segments[0].synced, 4);
        fs::write(state_path(dir.path(), 7), b"{not json").unwrap();
        assert!(load(dir.path(), 7).is_none());
        assert!(load(dir.path(), 8).is_none());
    }
}
