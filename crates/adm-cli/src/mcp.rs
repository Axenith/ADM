//! `adm-dl mcp`: ADM as a Model Context Protocol server over stdio, so an AI agent (Claude Code, opencode, Gemini
//! CLI, Codex, Cursor, ...) can download files with several connections and resume instead of a plain `curl`.
//!
//! A download is a background job. `adm_download` waits a bounded time for it (clients give up on a tool call after
//! a while) and, if the file is not done, answers with a job id that `adm_status` can wait on again.

use crate::jobs::{Manager, Request, Snap, human, state_dir};
use adm_core::{DownloadRequest, EngineConfig};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

const SUPPORTED: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const DEFAULT_WAIT: u64 = 45;
const MAX_WAIT: u64 = 300;

const INSTRUCTIONS: &str = "ADM downloads files over several connections at once and resumes after a failure. \
Use adm_download instead of curl or wget for anything bigger than a few MB (datasets, model weights, ISOs, \
releases, videos) or when a download is slow or keeps breaking. It never overwrites a file. If adm_download answers \
status \"running\", the file is still coming: call adm_status with the job_id and wait_seconds to wait for it. \
Pass sha256 when the publisher gives a checksum.";

struct Ctx {
    mgr: Arc<Manager>,
    default_dir: PathBuf,
    root: Option<PathBuf>,
    out: mpsc::UnboundedSender<String>,
}

pub async fn run() -> std::process::ExitCode {
    let mut cfg = EngineConfig::new(state_dir());
    if let Some(n) = env_num("ADM_CONNECTIONS") {
        cfg.max_segments = n as usize;
    }
    let max_jobs = env_num("ADM_MAX_JOBS").unwrap_or(4) as usize;
    let mgr = match Manager::new(cfg, max_jobs) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("adm-dl mcp: cannot start: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if let Some(kib) = env_num("ADM_LIMIT_KIB") {
        mgr.engine().speed_limiter().set_limit(kib * 1024);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
    let default_dir = std::env::var_os("ADM_DOWNLOAD_DIR").map(PathBuf::from).map(|p| absolute(&cwd, &p)).unwrap_or(cwd.clone());
    let root = std::env::var_os("ADM_MCP_ROOT").map(PathBuf::from).map(|p| absolute(&cwd, &p));

    let (out, mut lines) = mpsc::unbounded_channel::<String>();
    // One task owns stdout, so concurrent replies never interleave. Nothing but protocol goes there.
    let writer = tokio::spawn(async move {
        let mut so = tokio::io::stdout();
        while let Some(l) = lines.recv().await {
            if so.write_all(l.as_bytes()).await.is_err() || so.write_all(b"\n").await.is_err() || so.flush().await.is_err() {
                break;
            }
        }
    });

    let ctx = Arc::new(Ctx { mgr, default_dir, root, out });
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = input.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                ctx.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}));
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move { ctx.handle(msg).await });
    }
    // stdin closed: the client is gone. Dropping the runtime stops any download in flight; their state is saved
    // the next time they are started again.
    drop(ctx);
    let _ = tokio::time::timeout(Duration::from_millis(200), writer).await;
    std::process::ExitCode::SUCCESS
}

fn env_num(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok().filter(|n| *n > 0)
}

fn absolute(base: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    // `..` is resolved by hand: the folder may not exist yet.
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

impl Ctx {
    fn send(&self, v: Value) {
        let _ = self.out.send(v.to_string());
    }

    /// Tells the client how far the jobs it is waiting for have come, if it asked for that (`_meta.progressToken`).
    fn progress(&self, jobs: &[Arc<crate::jobs::Job>], token: &Option<Value>) {
        let Some(token) = token else { return };
        let snaps: Vec<Snap> = jobs.iter().map(|j| j.snap()).collect();
        let done: u64 = snaps.iter().map(|s| s.downloaded).sum();
        let total = snaps.iter().map(|s| s.total).sum::<Option<u64>>();
        let speed: f64 = snaps.iter().map(|s| s.speed).sum();
        let mut p = json!({ "progressToken": token, "progress": done, "message": format!("{} of {} at {}/s", human(done as f64), total.map_or("?".into(), |t| human(t as f64)), human(speed)) });
        if let Some(t) = total {
            p["total"] = json!(t);
        }
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": p }));
    }

    async fn handle(&self, msg: Value) {
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else { return };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else { return }; // a notification (initialized, cancelled, ...): nothing to answer
        let reply = match method {
            "initialize" => {
                let want = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if SUPPORTED.contains(&want) { want } else { SUPPORTED[1] };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "adm", "title": "ADM Download Manager", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(self.call(&params).await),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        self.send(match reply {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
        });
    }

    async fn call(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let token = params.get("_meta").and_then(|m| m.get("progressToken")).cloned();
        let res = match name {
            "adm_download" => self.download(&args, token).await,
            "adm_status" => self.status(&args, token).await,
            "adm_cancel" => self.cancel(&args),
            "adm_probe" => self.probe(&args).await,
            _ => Err(format!("unknown tool {name}")),
        };
        match res {
            Ok((v, is_error)) => result(v, is_error),
            Err(m) => result(json!({ "error": m }), true),
        }
    }

    /// Where a download goes: the given folder (relative ones start at the default), kept inside ADM_MCP_ROOT if set.
    fn dest(&self, dir: Option<&str>) -> Result<PathBuf, String> {
        let dest = match dir.filter(|d| !d.trim().is_empty()) {
            Some(d) => absolute(&self.default_dir, Path::new(d)),
            None => self.default_dir.clone(),
        };
        if let Some(root) = self.root.as_ref().filter(|r| !dest.starts_with(r)) {
            return Err(format!("dest_dir must be inside {} (set by ADM_MCP_ROOT)", root.display()));
        }
        std::fs::create_dir_all(&dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
        if let Some(root) = &self.root {
            // A symlink inside the root must not lead out of it.
            let real = dest.canonicalize().map_err(|e| e.to_string())?;
            let real_root = root.canonicalize().map_err(|e| e.to_string())?;
            if !real.starts_with(&real_root) {
                return Err(format!("dest_dir resolves outside {} (set by ADM_MCP_ROOT)", root.display()));
            }
        }
        Ok(dest)
    }

    async fn download(&self, args: &Value, token: Option<Value>) -> Reply {
        let mut urls: Vec<String> = Vec::new();
        if let Some(u) = args.get("url").and_then(Value::as_str) {
            urls.push(u.trim().to_owned());
        }
        if let Some(list) = args.get("urls").and_then(Value::as_array) {
            urls.extend(list.iter().filter_map(Value::as_str).map(|s| s.trim().to_owned()));
        }
        if urls.is_empty() {
            return Err("give a url, or a list of urls".into());
        }
        if urls.len() > 200 {
            return Err("at most 200 urls at once".into());
        }
        for u in &urls {
            check_url(u)?;
        }
        let single = urls.len() == 1 && args.get("urls").is_none();
        let name = args.get("file_name").and_then(Value::as_str).filter(|n| !n.is_empty());
        if name.is_some() && urls.len() > 1 {
            return Err("file_name only makes sense with a single url".into());
        }
        let dest = self.dest(args.get("dest_dir").and_then(Value::as_str))?;
        let headers = headers_arg(args)?;
        let connections = args.get("connections").and_then(Value::as_u64).map(|n| n.clamp(1, 32) as usize);
        let sha = args.get("sha256").and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase());
        if let Some(s) = &sha {
            if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("sha256 must be 64 hex characters".into());
            }
            if urls.len() > 1 {
                return Err("sha256 only makes sense with a single url".into());
            }
        }
        let wait = wait_arg(args, DEFAULT_WAIT);

        let jobs: Vec<_> = urls
            .into_iter()
            .map(|url| {
                self.mgr.start(Request {
                    url,
                    dest: dest.clone(),
                    file_name: name.map(adm_core::sanitize_file_name),
                    headers: headers.clone(),
                    connections,
                    sha256: sha.clone(),
                })
            })
            .collect();

        // One shared deadline for all of them; progress is the sum.
        let deadline = tokio::time::Instant::now() + wait;
        let mut snaps = Vec::new();
        for j in &jobs {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            snaps.push(self.mgr.wait(j, left, |_| self.progress(&jobs, &token)).await);
        }
        let views: Vec<Value> = jobs.iter().zip(&snaps).map(|(j, s)| s.to_json(&j.id, &j.url)).collect();
        let failed = snaps.iter().any(|s| matches!(s.phase, crate::jobs::Phase::Failed | crate::jobs::Phase::Cancelled));
        if single { Ok((views.into_iter().next().unwrap(), failed)) } else { Ok((json!({ "jobs": views }), failed)) }
    }

    async fn status(&self, args: &Value, token: Option<Value>) -> Reply {
        let wait = wait_arg(args, 0);
        match args.get("job_id").and_then(Value::as_str) {
            Some(id) => {
                let job = self.mgr.find(id).ok_or_else(|| format!("no job {id}. It may belong to an earlier session"))?;
                let one = [job.clone()];
                let snap = self.mgr.wait(&job, wait, |_| self.progress(&one, &token)).await;
                let bad = matches!(snap.phase, crate::jobs::Phase::Failed | crate::jobs::Phase::Cancelled);
                Ok((snap.to_json(&job.id, &job.url), bad))
            }
            None => Ok((json!({ "jobs": self.mgr.all().iter().map(|j| j.json()).collect::<Vec<_>>() }), false)),
        }
    }

    fn cancel(&self, args: &Value) -> Reply {
        let id = args.get("job_id").and_then(Value::as_str).ok_or("job_id is required")?;
        if self.mgr.cancel(id) {
            Ok((json!({ "job_id": id, "status": "cancelling" }), false))
        } else {
            Err(format!("job {id} does not exist or is already over"))
        }
    }

    async fn probe(&self, args: &Value) -> Reply {
        let url = args.get("url").and_then(Value::as_str).ok_or("url is required")?;
        check_url(url)?;
        let mut req = DownloadRequest::new(0, url, &self.default_dir);
        req.headers = headers_arg(args)?;
        let p = self.mgr.engine().probe(&req).await.map_err(|e| e.to_string())?;
        Ok((
            json!({ "file_name": p.file_name, "size_bytes": p.size, "content_type": p.content_type, "resumable": p.resumable, "final_url": p.final_url }),
            false,
        ))
    }
}

type Reply = Result<(Value, bool), String>;

fn result(v: Value, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": v.to_string() }], "structuredContent": v, "isError": is_error })
}

fn check_url(u: &str) -> Result<(), String> {
    let lower = u.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("only http:// and https:// links can be downloaded, got {u:?}"))
    }
}

fn wait_arg(args: &Value, default: u64) -> Duration {
    Duration::from_secs(args.get("wait_seconds").and_then(Value::as_u64).unwrap_or(default).min(MAX_WAIT))
}

fn headers_arg(args: &Value) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    if let Some(h) = args.get("headers") {
        let obj = h.as_object().ok_or("headers must be an object of name: value")?;
        for (k, v) in obj {
            let v = v.as_str().ok_or("header values must be strings")?;
            if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
                return Err("a header cannot contain a line break".into());
            }
            out.push((k.clone(), v.to_owned()));
        }
    }
    Ok(out)
}

fn tools() -> Value {
    json!([
        {
            "name": "adm_download",
            "title": "Download files fast",
            "description": "Download one or more files with ADM: several connections at once, automatic retries, resume after a failure, and a safe file name (never overwrites). Prefer this over curl/wget for anything bigger than a few MB, or when downloads are slow or fail. It waits up to wait_seconds (default 45); if the file is not finished by then it returns status \"running\" with a job_id, and you call adm_status to keep waiting. Returns the final path, size and speed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "http(s) link of the file." },
                    "urls": { "type": "array", "items": { "type": "string" }, "description": "Several links, downloaded in parallel (up to 200). Use instead of url." },
                    "dest_dir": { "type": "string", "description": "Folder to save into (created if missing). Relative paths start at the server's download folder. Default: that folder." },
                    "file_name": { "type": "string", "description": "Save under this name instead of the one the server gives. Single url only." },
                    "headers": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Extra request headers, for example Authorization, Cookie or Referer." },
                    "connections": { "type": "integer", "minimum": 1, "maximum": 32, "description": "Parallel connections per file. Default 8." },
                    "sha256": { "type": "string", "description": "Expected SHA-256 (hex). If the file does not match, the job fails and says so. Single url only." },
                    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 300, "description": "How long to wait for the file before returning a running job. Default 45." }
                }
            }
        },
        {
            "name": "adm_status",
            "title": "Check or wait for a download",
            "description": "Status of a download started with adm_download. With wait_seconds it waits for the download to finish (or that long) before answering, so use it instead of polling in a loop. Without job_id it lists every job of this session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "The job_id from adm_download. Omit to list all jobs." },
                    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 300, "description": "Wait up to this long for the job to finish. Default 0 (answer now)." }
                }
            }
        },
        {
            "name": "adm_cancel",
            "title": "Cancel a download",
            "description": "Stops a running or queued download and deletes the partial file.",
            "inputSchema": { "type": "object", "properties": { "job_id": { "type": "string" } }, "required": ["job_id"] }
        },
        {
            "name": "adm_probe",
            "title": "Inspect a link without downloading",
            "description": "Finds a link's file name, size, type and whether it can be resumed, by asking the server for its first byte. Use it to check a size before committing to a big download.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "headers": { "type": "object", "additionalProperties": { "type": "string" } }
                },
                "required": ["url"]
            }
        }
    ])
}
