//! `adm <url>`: a thin command-line front end over adm-core. Re-running the same command
//! resumes an interrupted download (Ctrl-C pauses and saves progress).

use adm_core::{DownloadRequest, Engine, EngineConfig, Event, Outcome};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: adm <url> [-o DIR] [-c CONNECTIONS] [--limit KIB_PER_SEC] [-H 'Name: value']... [--insecure]";

struct Args {
    url: String,
    dir: PathBuf,
    connections: usize,
    limit_kib: u64,
    headers: Vec<(String, String)>,
    insecure: bool,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        url: String::new(),
        dir: std::env::current_dir().map_err(|e| e.to_string())?,
        connections: 8,
        limit_kib: 0,
        headers: Vec::new(),
        insecure: false,
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "-o" => a.dir = value("-o")?.into(),
            "-c" => a.connections = value("-c")?.parse().map_err(|_| "-c needs a number")?,
            "--limit" => a.limit_kib = value("--limit")?.parse().map_err(|_| "--limit needs a number")?,
            "-H" => {
                let h = value("-H")?;
                let (k, v) = h.split_once(':').ok_or("-H needs 'Name: value'")?;
                a.headers.push((k.trim().into(), v.trim().into()));
            }
            "--insecure" => a.insecure = true,
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ if a.url.is_empty() => a.url = arg,
            _ => return Err(format!("unexpected argument {arg}\n{USAGE}")),
        }
    }
    if a.url.is_empty() {
        return Err(USAGE.into());
    }
    Ok(a)
}

/// Same URL and folder give the same id, so running the command again finds the saved state.
fn stable_id(url: &str, dir: &std::path::Path) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a
    for b in url.bytes().chain(dir.to_string_lossy().bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("adm").join("state")
}

fn human(n: f64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let (mut v, mut i) = (n, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

fn eta(secs: Option<u64>) -> String {
    match secs {
        Some(s) if s >= 3600 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        Some(s) if s >= 60 => format!("{}m{:02}s", s / 60, s % 60),
        Some(s) => format!("{s}s"),
        None => "--".into(),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(m) => {
            eprintln!("{m}");
            return ExitCode::from(2);
        }
    };
    let mut cfg = EngineConfig::new(state_dir());
    cfg.max_segments = args.connections;
    cfg.ignore_cert_errors = args.insecure;
    let engine = match Engine::new(cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("cannot start: {e}");
            return ExitCode::FAILURE;
        }
    };
    engine.speed_limiter().set_limit(args.limit_kib * 1024);

    let mut req = DownloadRequest::new(stable_id(&args.url, &args.dir), &args.url, &args.dir);
    req.headers = args.headers;
    let (handle, mut events) = engine.start(req);

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());
    let mut paused = false;
    loop {
        tokio::select! {
            _ = &mut ctrl_c, if !paused => { paused = true; eprintln!("\npausing..."); handle.pause(); }
            ev = events.recv() => match ev {
                Ok(Event::Init(i)) => eprintln!(
                    "{} ({}){}", i.file_name,
                    i.total_size.map_or("unknown size".into(), |s| human(s as f64)),
                    if i.resumable { "" } else { ", no resume support" }),
                Ok(Event::Progress(p)) => {
                    let pct = p.total.filter(|t| *t > 0).map_or(String::new(), |t| format!("{:5.1}%  ", p.downloaded as f64 * 100.0 / t as f64));
                    let active = p.segments.iter().filter(|s| s.downloaded < s.length).count();
                    eprint!("\r{pct}{}  {}/s  eta {}  [{active} conn]   ", human(p.downloaded as f64), human(p.speed), eta(p.eta_secs));
                    let _ = std::io::stderr().flush();
                }
                Ok(Event::Completed(_) | Event::Paused | Event::Failed(_)) | Err(_) => break,
            },
        }
    }
    match handle.wait().await {
        Outcome::Completed(f) => {
            eprintln!("\ndone: {} ({})", f.path.display(), human(f.size as f64));
            ExitCode::SUCCESS
        }
        Outcome::Paused => {
            eprintln!("paused; run the same command to resume");
            ExitCode::from(130)
        }
        Outcome::Failed(e) => {
            eprintln!("\nfailed: {e}");
            ExitCode::FAILURE
        }
    }
}
