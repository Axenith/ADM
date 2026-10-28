//! Lets the browser extension start ADM. A browser extension cannot run programs, but it can talk to a "native
//! messaging host" that the browser starts for it. ADM is its own host: the browser runs `adm chrome-extension://<id>/`,
//! the extension sends `{"op":"start"}`, and ADM starts itself (in the tray, no window) unless it is already running.
//! The browser finds the host through a small JSON file that `register` keeps up to date.

use std::io::{Read, Write};
use std::path::PathBuf;

pub const HOST_NAME: &str = "dev.adm.launcher";
/// Extensions allowed to start ADM: the one in this repository (its key is pinned in manifest.json).
const EXTENSION_IDS: &[&str] = &["hncgajannlpkmglkanljjngglcjhbdnm"];
/// Browser folders under the config directory that read `NativeMessagingHosts/`.
#[cfg(unix)]
const BROWSERS: &[&str] = &["google-chrome", "google-chrome-beta", "chromium", "BraveSoftware/Brave-Browser", "microsoft-edge", "vivaldi"];

/// Browsers start a native host with the calling extension's origin as the first argument.
pub fn is_host_launch(args: &[String]) -> bool {
    args.first().is_some_and(|a| a.starts_with("chrome-extension://"))
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

/// Starts ADM unless it is already answering. Ok(true) means it already was.
fn start() -> Result<bool, String> {
    if crate::server::is_adm() {
        return Ok(true);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--background").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // Its own process group, so it is not taken down when the browser closes the host.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    cmd.spawn().map(|_| false).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn manifest(exe: &std::path::Path) -> String {
    let origins: Vec<String> = EXTENSION_IDS.iter().map(|id| format!("chrome-extension://{id}/")).collect();
    let mut s = serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "Starts ADM when the browser extension needs it",
        "path": exe.display().to_string(),
        "type": "stdio",
        "allowed_origins": origins,
    }))
    .unwrap_or_default();
    s.push('\n');
    s
}

/// Tells every installed Chromium-family browser where this ADM is, so its extension can start it. Only browsers
/// that have a profile folder already are touched. Returns the files written (or already right).
#[cfg(unix)]
pub fn register() -> Vec<PathBuf> {
    let (Some(base), Ok(exe)) = (config_dir(), std::env::current_exe()) else { return Vec::new() };
    let want = manifest(&exe);
    let mut done = Vec::new();
    for b in BROWSERS {
        let root = base.join(b);
        if !root.is_dir() {
            continue;
        }
        let dir = root.join("NativeMessagingHosts");
        let file = dir.join(format!("{HOST_NAME}.json"));
        let right = std::fs::read_to_string(&file).is_ok_and(|have| have == want);
        if right || (std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, &want).is_ok()) {
            done.push(file);
        }
    }
    done
}

/// Removes what `register` wrote (used by the uninstaller).
#[cfg(unix)]
pub fn unregister() {
    let Some(base) = config_dir() else { return };
    for b in BROWSERS {
        let _ = std::fs::remove_file(base.join(b).join("NativeMessagingHosts").join(format!("{HOST_NAME}.json")));
    }
}

// ---- Windows ------------------------------------------------------------------------------------------------------
// Browsers find a host through the registry, which points at a JSON file. The host program must be a console program
// (a windowed one has no standard input or output), so it is `adm-dl.exe`, installed next to `adm.exe`: it answers the
// same protocol and starts `adm.exe`.

/// Registry keys (under HKEY_CURRENT_USER) that Chromium-family browsers read hosts from.
#[cfg(windows)]
const REGISTRY: &[&str] = &[
    r"Software\Google\Chrome\NativeMessagingHosts",
    r"Software\Chromium\NativeMessagingHosts",
    r"Software\Microsoft\Edge\NativeMessagingHosts",
    r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
];

#[cfg(windows)]
fn reg(args: &[&std::ffi::OsStr]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).output().is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
fn manifest_file() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("ADM").join(format!("{HOST_NAME}.json")))
}

/// Writes the host file and points each browser's registry key at it. Returns the file.
#[cfg(windows)]
pub fn register() -> Vec<PathBuf> {
    use std::ffi::OsStr;
    let (Some(file), Ok(exe)) = (manifest_file(), std::env::current_exe()) else { return Vec::new() };
    let host = exe.with_file_name("adm-dl.exe");
    let host = if host.exists() { host } else { exe };
    let want = manifest(&host);
    if std::fs::read_to_string(&file).map_or(true, |have| have != want) {
        if file.parent().is_none_or(|d| std::fs::create_dir_all(d).is_err()) || std::fs::write(&file, &want).is_err() {
            return Vec::new();
        }
    }
    for base in REGISTRY {
        let key = format!(r"HKCU\{base}\{HOST_NAME}");
        reg(&[OsStr::new("add"), OsStr::new(&key), OsStr::new("/ve"), OsStr::new("/t"), OsStr::new("REG_SZ"), OsStr::new("/d"), file.as_os_str(), OsStr::new("/f")]);
    }
    vec![file]
}

/// Removes the registry keys and the host file (used by the uninstaller).
#[cfg(windows)]
pub fn unregister() {
    use std::ffi::OsStr;
    for base in REGISTRY {
        let key = format!(r"HKCU\{base}\{HOST_NAME}");
        reg(&[OsStr::new("delete"), OsStr::new(&key), OsStr::new("/f")]);
    }
    if let Some(file) = manifest_file() {
        let _ = std::fs::remove_file(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_name_the_extension_by_origin() {
        assert!(is_host_launch(&["chrome-extension://abc/".into()]));
        assert!(!is_host_launch(&["https://example.com/file.zip".into()]));
        assert!(!is_host_launch(&[]));
    }

    #[test]
    fn the_manifest_points_at_this_program_and_only_our_extension() {
        let m: serde_json::Value = serde_json::from_str(&manifest(std::path::Path::new("/opt/adm"))).unwrap();
        assert_eq!(m["name"], HOST_NAME);
        assert_eq!(m["path"], "/opt/adm");
        assert_eq!(m["type"], "stdio");
        let origins = m["allowed_origins"].as_array().unwrap();
        assert_eq!(origins.len(), EXTENSION_IDS.len());
        assert!(origins.iter().all(|o| o.as_str().unwrap().starts_with("chrome-extension://") && o.as_str().unwrap().ends_with('/')));
    }
}
