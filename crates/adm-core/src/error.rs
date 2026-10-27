use serde::{Deserialize, Serialize};

/// Why a download stopped. Mirrors the categories the UI needs to tell apart: some are worth a
/// retry, some need a fresh link, some need the user to free disk space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum DownloadError {
    #[error("network error")]
    Network,
    #[error("the server sent an invalid or refused response")]
    InvalidResponse,
    #[error("session expired")]
    SessionExpired,
    /// Part of the file arrived, then the server started refusing the link (403/410/401).
    #[error("the link expired; refresh it to resume")]
    LinkExpired,
    #[error("disk error (full, missing or read-only)")]
    Disk,
    #[error("internal error")]
    Internal,
    /// The server ignored a range request, or the file changed since the download started.
    #[error("the server does not support resuming, or the file changed")]
    ResumeNotSupported,
    #[error("TLS certificate could not be verified")]
    Tls,
    /// The finished file could not be moved to its destination.
    #[error("could not write the finished file")]
    OutputWrite,
}

pub type Result<T> = std::result::Result<T, DownloadError>;
