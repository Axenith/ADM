//! The browser extension's way to start ADM (native messaging), for platforms where the app itself cannot be the host.
//! On Windows the app is a windowed program with no standard input or output, so the browser runs this console program
//! instead (`adm-dl.exe chrome-extension://<id>/`, installed next to `adm.exe`). It answers the same one message as the
//! app's own host on Linux (see src-tauri/src/native_host.rs): `{"op":"start"}` starts ADM in the tray unless it is
//! already answering.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// A browser starts a native host with the calling extension's origin as the first argument.
pub fn is_host_launch(first_arg: Option<&str>) -> bool {
    first_arg.is_some_and(|a| a.starts_with("chrome-extension://"))
}

fn read_message() -> Option<serde_json::Value> {
    let mut stdin = std::io::stdin().lock();
    let mut len = [0u8; 4];
    stdin.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 64 * 1024 {
        return None;
    }
    let mut body = vec![0u8; len];
    stdin.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn reply(v: serde_json::Value) {
    let body = v.to_string();
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&(body.len() as u32).to_le_bytes());
    let _ = out.write_all(body.as_bytes());
    let _ = out.flush();
}

/// Answers one message and exits. Nothing but the protocol may be written to stdout.
pub fn run() {
    let op = read_message().and_then(|m| m.get("op").and_then(|o| o.as_str()).map(str::to_owned));
    match op.as_deref() {
        Some("start") => reply(match start() {
            Ok(already_running) => serde_json::json!({ "ok": true, "running": already_running }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        }),
        _ => reply(serde_json::json!({ "ok": false, "error": "unknown request" })),
    }
}

fn port() -> u16 {
    std::env::var("ADM_PORT").ok().and_then(|p| p.trim().parse().ok()).unwrap_or(8598)
}

/// Whether ADM itself (not some other program on the port) answers.
fn is_adm() -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port()));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_millis(1500)));
    let req = format!("GET /sync HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", port());
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).contains("\"app\":\"adm\"")
}

/// Starts `adm` (the program next to this one) in the tray, unless it already answers. Ok(true) = it already did.
fn start() -> Result<bool, String> {
    if is_adm() {
        return Ok(true);
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let app = me.with_file_name(if cfg!(windows) { "adm.exe" } else { "adm" });
    let mut cmd = std::process::Command::new(app);
    cmd.arg("--background").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // Not tied to the browser: it must outlive this host.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    cmd.spawn().map(|_| false).map_err(|e| format!("could not start ADM: {e}"))
}
