//! `adm-dl <url>`: a thin command-line front end over adm-core. Re-running the same command
//! resumes an interrupted download (Ctrl-C pauses and saves progress). `adm-dl mcp` serves the same engine to
//! AI agents over the Model Context Protocol.

mod host;
mod jobs;
mod mcp;

use adm_core::{DownloadRequest, Engine, EngineConfig, Event, Outcome};
use jobs::{human, sha256_file, stable_id, state_dir};
use serde_json::json;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: adm-dl <url> [-o DIR] [-n NAME] [-c CONNECTIONS] [--limit KIB_PER_SEC] [-H 'Name: value']... [--sha256 HEX] [--json] [--insecure]
       adm-dl mcp        serve ADM to AI agents (Model Context Protocol, stdio)

  --json     print one JSON line on stdout when done: {\"status\":\"completed\",\"path\":...,\"size_bytes\":...}
  exit code  0 done, 1 failed, 2 bad usage, 130 paused (run the same command to resume)";

struct Args {
    url: String,
    dir: PathBuf,
    connections: usize,
    limit_kib: u64,
    headers: Vec<(String, String)>,
    insecure: bool,
    name: Option<String>,
    sha256: Option<String>,
    json: bool,
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
        name: None,
        sha256: None,
        json: false,
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
            "-n" | "--name" => a.name = Some(adm_core::sanitize_file_name(&value("-n")?)),
            "--sha256" => {
                let h = value("--sha256")?.to_ascii_lowercase();
                if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("--sha256 needs 64 hex characters".into());
                }
                a.sha256 = Some(h);
            }
            "--json" => a.json = true,
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
    let first = std::env::args().nth(1);
    if first.as_deref() == Some("mcp") {
        return mcp::run().await;
    }
    // The browser extension starting ADM (see host.rs): answer its one message and leave.
    if host::is_host_launch(first.as_deref()) {
        host::run();
        return ExitCode::SUCCESS;
    }
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

    // A terminal gets one live line; anything else (an agent, a log file) gets a plain line every few seconds
    // instead of a carriage-return storm.
    let live = std::io::stderr().is_terminal();
    let mut last_line = Instant::now() - Duration::from_secs(60);

    let mut req = DownloadRequest::new(stable_id(&args.url, &args.dir), &args.url, &args.dir);
    req.headers = args.headers;
    req.file_name = args.name;
    let started = Instant::now();
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
                    if live {
                        eprint!("\r{pct}{}  {}/s  eta {}  [{active} conn]   ", human(p.downloaded as f64), human(p.speed), eta(p.eta_secs));
                        let _ = std::io::stderr().flush();
                    } else if last_line.elapsed() >= Duration::from_secs(5) {
                        last_line = Instant::now();
                        eprintln!("{}{}  {}/s  eta {}  [{active} conn]", pct.trim_start(), human(p.downloaded as f64), human(p.speed), eta(p.eta_secs));
                    }
                }
                Ok(Event::Completed(_) | Event::Paused | Event::Failed(_)) | Err(_) => break,
            },
        }
    }
    match handle.wait().await {
        Outcome::Completed(f) => {
            let mut sha = None;
            if let Some(want) = &args.sha256 {
                let path = f.path.clone();
                let got = tokio::task::spawn_blocking(move || sha256_file(&path)).await.ok().and_then(Result::ok);
                if got.as_deref() != Some(want.as_str()) {
                    let msg = format!("checksum mismatch: expected {want}, the file has {}. It is kept at {}; do not use it", got.as_deref().unwrap_or("(unreadable)"), f.path.display());
                    if args.json {
                        println!("{}", json!({ "status": "failed", "error": msg, "path": f.path.display().to_string(), "sha256": got }));
                    }
                    eprintln!("\nfailed: {msg}");
                    return ExitCode::FAILURE;
                }
                sha = got;
            }
            let secs = started.elapsed().as_secs_f64();
            eprintln!("\ndone: {} ({})", f.path.display(), human(f.size as f64));
            if args.json {
                println!("{}", json!({
                    "status": "completed", "path": f.path.display().to_string(), "size_bytes": f.size, "sha256": sha,
                    "seconds": (secs * 10.0).round() / 10.0, "avg_speed_bytes_per_sec": if secs > 0.0 { (f.size as f64 / secs).round() } else { 0.0 },
                }));
            }
            ExitCode::SUCCESS
        }
        Outcome::Paused => {
            eprintln!("paused; run the same command to resume");
            if args.json {
                println!("{}", json!({ "status": "paused", "hint": "run the same command to resume" }));
            }
            ExitCode::from(130)
        }
        Outcome::Failed(e) => {
            eprintln!("\nfailed: {e}");
            if args.json {
                println!("{}", json!({ "status": "failed", "error": e.to_string() }));
            }
            ExitCode::FAILURE
        }
    }
}
