//! The loopback server the browser extension talks to, and the single-instance lock.
//!
//! The port doubles as the lock: the OS frees it on a crash, so there is no stale lock file to reason
//! about, and it is already the channel for handing work to the running instance. A second launch finds
//! the port taken, asks the first instance to show itself (passing along any link it was given) and exits.
//!
//! The protocol follows XDM's (`/sync`, `/download`, `/poll`, `/show`, ...) but ADM listens on its own port,
//! so ADM and XDM can run side by side: the lock keeps working while XDM is open, and each extension talks
//! to its own app. The guards are XDM's too: a web page can reach `127.0.0.1` as easily as the extension can,
//! so a request is refused unless it names this server in `Host` (DNS rebinding), carries no `Origin` or a
//! browser-extension one (a page's own origin is `http(s)://...`), and, for anything that
//! changes state, is a POST (a page can fire simple GETs, e.g. `<img src>`).

use crate::store::BatchItem;
use crate::{Extra, Shared, request_batch, request_download, show_main};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener as StdListener, TcpStream as StdStream};
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// ADM's port. `ADM_PORT` overrides it, for running a second, separate copy (the end-to-end tests do).
pub const DEFAULT_PORT: u16 = 8598;

pub fn port() -> u16 {
    static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *PORT.get_or_init(|| std::env::var("ADM_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PORT))
}

const MAX_HEAD: usize = 32 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `/poll` parks before telling the extension there is nothing new.
const POLL_WAIT: Duration = Duration::from_secs(25);

const EXTENSION_ORIGINS: [&str; 4] = ["chrome-extension://", "moz-extension://", "extension://", "safari-web-extension://"];
const STATE_CHANGING: [&str; 9] = ["/download", "/media", "/vid", "/batch", "/clear", "/clear-tab", "/tab-update", "/show", "/poll"];

/// Headers the extension captured that must not be replayed: they describe the browser's own request
/// (conditionals would turn the fetch into a bodiless 304; `Range` and `Accept-Encoding` are ours to set).
const BLOCKED_HEADERS: [&str; 11] = [
    "accept", "connection", "expect", "te", "upgrade", "range", "cookie", "transfer-encoding", "content-type",
    "content-length", "content-encoding",
];
const BLOCKED_PREFIXES: [&str; 3] = ["if-", "proxy-", "accept-encoding"];

const BLOCKED_HOSTS: &[&str] = &["update.microsoft.com", "windowsupdate.com", "thwawte.com"];

pub enum Acquired {
    /// This process owns the port: it is the one ADM.
    Primary(StdListener),
    /// Another ADM owns it and has been asked to show itself; this process should exit.
    AnotherInstance,
    /// Something that is not ADM (maybe XDM, which uses the same port) holds it.
    PortTaken,
}

pub fn acquire(args: &[String]) -> Acquired {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    if let Ok(l) = StdListener::bind(addr) {
        return Acquired::Primary(l);
    }
    if !is_adm() {
        return Acquired::PortTaken;
    }
    let body = json!({ "args": args }).to_string();
    if matches!(local_request("POST", "/show", &body), Some((200, _))) {
        return Acquired::AnotherInstance;
    }
    // The instance we probed went away between the probe and the request: one more try for the port.
    match StdListener::bind(addr) {
        Ok(l) => Acquired::Primary(l),
        Err(_) => Acquired::PortTaken,
    }
}

fn is_adm() -> bool {
    matches!(local_request("GET", "/sync", ""), Some((200, body)) if body.contains("\"app\":\"adm\""))
}

/// A tiny blocking HTTP client for the hand-off; it must not touch any configured proxy.
fn local_request(method: &str, path: &str, body: &str) -> Option<(u16, String)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    let mut s = StdStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
    s.set_write_timeout(Some(Duration::from_millis(1500))).ok()?;
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        port(),
        body.len()
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text.split_whitespace().nth(1)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").map_or(String::new(), |(_, b)| b.to_string());
    Some((status, body))
}

pub async fn serve(listener: StdListener, app: AppHandle, st: Shared) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    let Ok(listener) = TcpListener::from_std(listener) else { return };
    let instance = format!("{:x}", crate::new_id());
    loop {
        let Ok((conn, _)) = listener.accept().await else { continue };
        let (app, st, instance) = (app.clone(), st.clone(), instance.clone());
        tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(IO_TIMEOUT + POLL_WAIT, handle(conn, app, st, &instance)).await;
        });
    }
}

struct Head {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

impl Head {
    fn parse(raw: &str) -> Option<Head> {
        let mut lines = raw.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, target) = (first.next()?.to_string(), first.next()?);
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        Some(Head { method, path: target.split('?').next()?.to_string(), headers })
    }

    /// The refusal for a request that must not be served, before its body is read.
    fn rejection(&self) -> Option<(u16, &'static str)> {
        let host_ok = self.headers.get("host").is_some_and(|h| {
            h.eq_ignore_ascii_case(&format!("127.0.0.1:{}", port())) || h.eq_ignore_ascii_case(&format!("localhost:{}", port()))
        });
        if !host_ok {
            return Some((403, "Forbidden"));
        }
        if let Some(origin) = self.headers.get("origin") {
            let lower = origin.to_ascii_lowercase();
            if !EXTENSION_ORIGINS.iter().any(|p| lower.starts_with(p)) {
                return Some((403, "Forbidden"));
            }
        }
        if STATE_CHANGING.contains(&self.path.as_str()) && self.method != "POST" {
            return Some((405, "Method Not Allowed"));
        }
        None
    }
}

async fn respond(s: &mut TcpStream, code: u16, reason: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nCache-Control: max-age=0, no-cache, must-revalidate\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes()).await;
    let _ = s.write_all(body.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// Reads the request head and body, or `None` if it is malformed, too large, or too slow.
async fn read_request(s: &mut TcpStream) -> Option<Result<(Head, Vec<u8>), (u16, &'static str)>> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Some(Err((431, "Request Header Fields Too Large")));
        }
        let n = tokio::time::timeout(IO_TIMEOUT, s.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = Head::parse(&String::from_utf8_lossy(&buf[..end]))?;
    // Refused on the head alone, before any body is read.
    if let Some(r) = head.rejection() {
        return Some(Err(r));
    }
    let len: usize = head.headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Some(Err((413, "Payload Too Large")));
    }
    let mut body = buf[end + 4..].to_vec();
    while body.len() < len {
        let n = tokio::time::timeout(IO_TIMEOUT, s.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Some(Ok((head, body)))
}

/// What the extension sends with a captured download. Unknown fields are ignored, and numbers may arrive
/// as numbers or strings, so values stay loose.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Message {
    url: Option<String>,
    cookie: Option<String>,
    request_headers: Option<HashMap<String, Vec<String>>>,
    filename: Option<String>,
    user_agent: Option<String>,
    tab_url: Option<String>,
    referer: Option<String>,
    args: Option<Vec<String>>,
    file_size: Option<Value>,
}

impl Message {
    fn into_extra(self) -> Option<(String, Extra)> {
        let url = self.url?;
        let mut headers: Vec<(String, String)> = self
            .request_headers
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, _)| {
                let k = k.to_ascii_lowercase();
                !BLOCKED_HEADERS.contains(&k.as_str()) && !BLOCKED_PREFIXES.iter().any(|p| k.starts_with(p))
            })
            .map(|(k, v)| (k, v.join(", ")))
            .collect();
        let has = |h: &[(String, String)], name: &str| h.iter().any(|(k, _)| k.eq_ignore_ascii_case(name));
        let page = self.referer.clone().or(self.tab_url.clone());
        if !has(&headers, "referer") {
            if let Some(r) = page.clone() {
                headers.push(("Referer".into(), r));
            }
        }
        if !has(&headers, "user-agent") {
            if let Some(ua) = self.user_agent {
                headers.push(("User-Agent".into(), ua));
            }
        }
        // Chrome's suggested name may carry sub-directories ("sub/file.zip"): keep the base name.
        let file_name = self.filename.filter(|f| !f.trim().is_empty()).map(|f| adm_core::sanitize_file_name(&f));
        let size = self.file_size.as_ref().and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())).filter(|n| *n > 0);
        Some((url, Extra { headers, cookie: self.cookie.filter(|c| !c.is_empty()), file_name, size, page }))
    }
}

/// `POST /batch`: several links at once ("Grab all links"). At most `MAX_BATCH` are taken.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct BatchMessage {
    items: Vec<BatchMessageItem>,
    referer: Option<String>,
    tab_url: Option<String>,
    user_agent: Option<String>,
    cookie: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct BatchMessageItem {
    url: String,
    filename: Option<String>,
    file_size: Option<Value>,
}

const MAX_BATCH: usize = 200;

impl BatchMessage {
    /// http(s) links only, each once, at most `MAX_BATCH`; the shared context as for a single download.
    fn into_parts(self) -> Option<(Vec<BatchItem>, Extra)> {
        let mut seen = std::collections::HashSet::new();
        let items: Vec<BatchItem> = self
            .items
            .into_iter()
            .map(|i| BatchItem {
                url: i.url.trim().to_string(),
                name: i.filename.filter(|f| !f.trim().is_empty()).map(|f| page_file_name(&f)),
                size: i.file_size.as_ref().and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())).filter(|n| *n > 0),
            })
            .filter(|i| (i.url.starts_with("http://") || i.url.starts_with("https://")) && seen.insert(i.url.clone()))
            .take(MAX_BATCH)
            .collect();
        if items.is_empty() {
            return None;
        }
        let page = self.referer.clone().or(self.tab_url.clone());
        let mut headers = Vec::new();
        if let Some(r) = page.clone() {
            headers.push(("Referer".to_string(), r));
        }
        if let Some(ua) = self.user_agent.filter(|u| !u.is_empty()) {
            headers.push(("User-Agent".to_string(), ua));
        }
        Some((items, Extra { headers, cookie: self.cookie.filter(|c| !c.is_empty()), file_name: None, size: None, page }))
    }
}

fn page_file_name(f: &str) -> String {
    adm_core::sanitize_file_name(f)
}

fn sync_body(instance: &str, blocked: &[String], file_exts: &[String], capture_over_mb: u64) -> String {
    // The media lists stay empty until video capture exists: with them set, the extension would send a
    // message for every media request it sees and nothing would use it.
    json!({
        "app": "adm",
        "enabled": true,
        "fileExts": file_exts,
        "captureOverMB": capture_over_mb,
        "blockedHosts": BLOCKED_HOSTS.iter().map(|h| h.to_string()).chain(blocked.iter().cloned()).collect::<Vec<_>>(),
        "requestFileExts": [],
        "mediaTypes": [],
        "matchingHosts": [],
        "videoList": [],
        "version": 0,
        "instanceId": instance,
    })
    .to_string()
}

async fn handle(mut s: TcpStream, app: AppHandle, st: Shared, instance: &str) {
    let (head, body) = match read_request(&mut s).await {
        Some(Ok(r)) => r,
        Some(Err((code, reason))) => return respond(&mut s, code, reason, "").await,
        None => return,
    };
    let msg: Message = serde_json::from_slice(&body).unwrap_or_default();
    match head.path.as_str() {
        "/sync" => {}
        "/poll" => {
            // Nothing changes yet that the extension needs to hear about: park, then say so.
            tokio::time::sleep(POLL_WAIT).await;
            return respond(&mut s, 204, "No Content", "").await;
        }
        "/download" => {
            if let Some((url, extra)) = msg.into_extra() {
                // Waits in the "new download" dialog (or starts at once, per the setting); either way show ADM.
                if request_download(&app, &st, &url, extra, None).is_ok() {
                    show_main(&app);
                }
            }
        }
        "/show" => {
            show_main(&app);
            for url in msg.args.unwrap_or_default().iter().filter(|a| a.starts_with("http")) {
                let _ = request_download(&app, &st, url, Extra::default(), None);
            }
        }
        "/batch" => {
            if let Some((items, extra)) = serde_json::from_slice::<BatchMessage>(&body).unwrap_or_default().into_parts() {
                if request_batch(&app, &st, items, extra, None).is_ok() {
                    show_main(&app);
                }
            }
        }
        // Accepted so a client that sends them is satisfied; ADM has no use for these.
        "/media" | "/vid" | "/clear" | "/clear-tab" | "/tab-update" => {}
        _ => return respond(&mut s, 404, "Not Found", "").await,
    }
    let (blocked, exts, over) = {
        let g = st.inner.lock().unwrap();
        (g.db.settings.blocked_sites.clone(), g.db.settings.capture_extensions.clone(), g.db.settings.capture_over_mb)
    };
    respond(&mut s, 200, "OK", &sync_body(instance, &blocked, &exts, over)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(method: &str, path: &str, extra: &[(&str, &str)]) -> Head {
        let mut headers: HashMap<String, String> = extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        headers.entry("host".into()).or_insert(format!("127.0.0.1:{}", port()));
        Head { method: method.into(), path: path.into(), headers }
    }

    #[test]
    fn guards() {
        assert!(head("GET", "/sync", &[]).rejection().is_none());
        assert!(head("POST", "/download", &[("origin", "chrome-extension://abc")]).rejection().is_none());
        assert!(head("POST", "/download", &[("origin", "moz-extension://abc")]).rejection().is_none());
        // A web page: its own origin, or null from a sandboxed frame.
        assert_eq!(head("POST", "/download", &[("origin", "https://evil.example")]).rejection().unwrap().0, 403);
        assert_eq!(head("POST", "/download", &[("origin", "null")]).rejection().unwrap().0, 403);
        // DNS rebinding: the page's own host name arrives in Host.
        assert_eq!(head("GET", "/sync", &[("host", "evil.example:9")]).rejection().unwrap().0, 403);
        assert_eq!(head("GET", "/sync", &[("host", &*format!("localhost:{}", port()))]).rejection(), None);
        // A simple GET (e.g. <img src>) must not change state.
        assert_eq!(head("GET", "/download", &[]).rejection().unwrap().0, 405);
        assert_eq!(head("GET", "/show", &[]).rejection().unwrap().0, 405);
    }

    #[test]
    fn message_to_download() {
        let m: Message = serde_json::from_str(
            r#"{"url":"https://x.io/a.zip","cookie":"s=1","filename":"sub/dir/a.zip","tabUrl":"https://x.io/page",
                "requestHeaders":{"Referer":["https://x.io/p"],"Range":["bytes=0-"],"If-None-Match":["x"],"X-Custom":["a","b"]},
                "fileSize":12,"somethingNew":true}"#,
        )
        .unwrap();
        let (url, e) = m.into_extra().unwrap();
        assert_eq!(url, "https://x.io/a.zip");
        assert_eq!(e.file_name.as_deref(), Some("a.zip"));
        assert_eq!(e.cookie.as_deref(), Some("s=1"));
        let names: Vec<_> = e.headers.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
        assert!(names.contains(&"referer".to_string()) && names.contains(&"x-custom".to_string()));
        assert!(!names.contains(&"range".to_string()) && !names.contains(&"if-none-match".to_string()));
        assert_eq!(e.headers.iter().find(|(k, _)| k == "X-Custom").unwrap().1, "a, b");
    }

    #[test]
    fn a_batch_is_cleaned_before_it_is_used() {
        let m: BatchMessage = serde_json::from_str(
            r#"{"items":[{"url":"https://x.io/a.zip","filename":"dir/A File.zip","fileSize":"1234"},{"url":"https://x.io/a.zip"},{"url":"javascript:alert(1)"},
                         {"url":"ftp://x.io/f"},{"url":"  https://x.io/b.pdf  "},{"url":""}],
                "referer":"https://x.io/page","userAgent":"UA/1","cookie":"s=1"}"#,
        )
        .unwrap();
        let (items, extra) = m.into_parts().unwrap();
        assert_eq!(items.iter().map(|i| i.url.as_str()).collect::<Vec<_>>(), ["https://x.io/a.zip", "https://x.io/b.pdf"], "http(s) only, each once, trimmed");
        assert_eq!((items[0].name.as_deref(), items[0].size), (Some("A File.zip"), Some(1234)));
        assert_eq!(extra.cookie.as_deref(), Some("s=1"));
        assert_eq!(extra.page.as_deref(), Some("https://x.io/page"));
        assert!(extra.headers.iter().any(|(k, v)| k == "Referer" && v == "https://x.io/page") && extra.headers.iter().any(|(k, _)| k == "User-Agent"));
        let none: BatchMessage = serde_json::from_str(r#"{"items":[{"url":"file:///etc/passwd"}]}"#).unwrap();
        assert!(none.into_parts().is_none());
        let many = format!(r#"{{"items":[{}]}}"#, (0..500).map(|i| format!(r#"{{"url":"https://x.io/{i}"}}"#)).collect::<Vec<_>>().join(","));
        assert_eq!(serde_json::from_str::<BatchMessage>(&many).unwrap().into_parts().unwrap().0.len(), 200, "capped");
    }

    #[test]
    fn sync_publishes_the_users_capture_rules_and_blocked_sites() {
        let body: Value = serde_json::from_str(&sync_body("i1", &["mine.example".into()], &["ZIP".into(), "ISO".into()], 50)).unwrap();
        assert_eq!(body["app"], "adm");
        assert_eq!(body["fileExts"], json!(["ZIP", "ISO"]));
        assert_eq!(body["captureOverMB"], 50);
        assert_eq!(body["blockedHosts"].as_array().unwrap().last().unwrap(), "mine.example");
        assert!(body["blockedHosts"].as_array().unwrap().iter().any(|h| h == "update.microsoft.com"), "built-in ones stay");
    }

    #[test]
    fn parses_request_head() {
        let h = Head::parse("POST /download?x=1 HTTP/1.1\r\nHost: 127.0.0.1:8598\r\nContent-Length: 5").unwrap();
        assert_eq!((h.method.as_str(), h.path.as_str()), ("POST", "/download"));
        assert_eq!(h.headers["content-length"], "5");
    }
}
