use serde::{Deserialize, Serialize};

/// Why a download stopped. Mirrors the categories the UI needs to tell apart: some are worth a
/// retry, some need a fresh link, some need the user to free disk space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum DownloadError {
    #[error("network error")]
    Network,
    /// A reply that does not make sense (e.g. a part of the file from the wrong place).
    #[error("the server sent a reply ADM could not use")]
    InvalidResponse,
    /// The server answered with an HTTP status that asking again will not change (404, 403, ...).
    #[error("{}", http_message(*.0))]
    Http(u16),
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

/// What an HTTP status means to someone who just wants their file, with the code kept for the curious.
pub fn http_message(code: u16) -> String {
    match code {
        400 => "The server rejected the request (HTTP 400)".into(),
        401 => "The server wants you to sign in (HTTP 401)".into(),
        403 => "Access denied (HTTP 403). The link may need you to be signed in, or may have expired".into(),
        404 => "Not found on the server (HTTP 404)".into(),
        410 => "The file is no longer available (HTTP 410)".into(),
        416 => "The server could not give the requested part of the file (HTTP 416)".into(),
        500..=599 => format!("The server had an error (HTTP {code})"),
        _ => format!("The server answered with HTTP {code}"),
    }
}

pub type Result<T> = std::result::Result<T, DownloadError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_errors_say_what_the_server_answered() {
        assert_eq!(DownloadError::Http(404).to_string(), "Not found on the server (HTTP 404)");
        assert!(DownloadError::Http(403).to_string().contains("HTTP 403") && DownloadError::Http(403).to_string().contains("signed in"));
        assert_eq!(DownloadError::Http(503).to_string(), "The server had an error (HTTP 503)");
        assert_eq!(DownloadError::Http(418).to_string(), "The server answered with HTTP 418");
        // The code survives being saved and sent to the interface.
        let json = serde_json::to_string(&DownloadError::Http(410)).unwrap();
        assert_eq!(serde_json::from_str::<DownloadError>(&json).unwrap(), DownloadError::Http(410));
    }
}
