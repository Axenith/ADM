# ADM for AI agents

AI coding agents download files with `curl` or `wget`: one connection, no resume, and no way to see progress.
ADM gives an agent the same engine the app uses: several connections at once, retries, resume, a checksum check, and
a file that never overwrites another. It needs **no GUI and no running ADM app**, so it works over SSH, in a
container and in CI.

There are two ways in. Pick the one your agent supports.

| | |
|---|---|
| **MCP server** `adm-dl mcp` | Claude Code, opencode, Gemini CLI, Codex CLI, Cursor, Claude Desktop, and anything else that speaks the Model Context Protocol over stdio. The agent sees four tools. |
| **Command line** `adm-dl <url> --json` | Any agent that can run a shell command. One JSON line on stdout when done. |

## Set up the MCP server

Install ADM (`adm-dl` goes to `~/.local/bin`), then tell your agent where it is.

**Claude Code**
```sh
claude mcp add adm -- adm-dl mcp
```

**opencode** (`opencode.json`)
```json
{ "$schema": "https://opencode.ai/config.json",
  "mcp": { "adm": { "type": "local", "command": ["adm-dl", "mcp"], "enabled": true } } }
```

**Gemini CLI** (`~/.gemini/settings.json`), **Cursor** (`.cursor/mcp.json`), **Claude Desktop**
```json
{ "mcpServers": { "adm": { "command": "adm-dl", "args": ["mcp"] } } }
```

**Codex CLI** (`~/.codex/config.toml`)
```toml
[mcp_servers.adm]
command = "adm-dl"
args = ["mcp"]
```

ChatGPT connects to remote MCP servers over HTTPS, and `adm-dl mcp` speaks stdio, so it cannot use it yet (an HTTP
transport would fix that). Any agent that can run a shell command can use the command line below.

Settings, as environment variables on the server (all optional):

| Variable | Meaning |
|---|---|
| `ADM_DOWNLOAD_DIR` | Where files go when the agent gives no `dest_dir`. Default: the folder the agent was started in. |
| `ADM_MCP_ROOT` | Keep every download inside this folder. A `dest_dir` outside it (or a symlink leading out) is refused. **Set this if you do not fully trust what the agent reads**: a web page can try to talk an agent into saving a file somewhere. |
| `ADM_CONNECTIONS` | Connections per file (default 8). |
| `ADM_MAX_JOBS` | Files downloading at once (default 4); the rest wait. |
| `ADM_LIMIT_KIB` | Total speed limit in KiB/s. |

## The tools

| Tool | What it does |
|---|---|
| `adm_download` | Downloads `url`, or a list of `urls` in parallel. Options: `dest_dir`, `file_name`, `headers` (Authorization, Cookie, Referer), `connections`, `sha256`, `wait_seconds`. |
| `adm_status` | Status of a job. With `wait_seconds` it waits for the download to end instead of the agent polling. Without `job_id` it lists the session's jobs. |
| `adm_cancel` | Stops a job and deletes the partial file. |
| `adm_probe` | A link's name, size, type and whether it can be resumed, without downloading it. |

**Long downloads.** Agent clients give up on a tool call after a while (often a minute), and a model file can take
much longer. So `adm_download` waits `wait_seconds` (default 45) and, if the file is not done, answers
`{"status": "running", "job_id": ...}` with progress; the agent calls `adm_status` with that `job_id` and
`wait_seconds` to keep waiting. The download goes on in between. Clients that send a progress token get live
progress notifications while a call waits.

A finished download answers with the final `path` (it may be `name (1).ext` if the name was taken), `size_bytes`,
`seconds` and `avg_speed_bytes_per_sec`. With `sha256`, a file that does not match fails the job with both hashes and
says not to use it (the file is kept so you can look at it). Asking for a link that is already downloading into the
same folder returns the same job.

## The command line

```sh
adm-dl https://example.com/model.safetensors -o ./models --sha256 <hex> --json
```

Progress goes to stderr (one plain line every 5 seconds when it is not a terminal), and on stdout one line:

```json
{"status":"completed","path":"/work/models/model.safetensors","size_bytes":4294967296,"sha256":"…","seconds":38.2,"avg_speed_bytes_per_sec":112430000}
```

Exit code `0` done, `1` failed (`{"status":"failed","error":"Not found on the server (HTTP 404)"}`), `2` bad usage,
`130` paused. Running the same command again resumes. Options: `-o DIR`, `-n NAME`, `-c CONNECTIONS`,
`--limit KIB_PER_SEC`, `-H 'Name: value'`, `--sha256 HEX`, `--insecure`.

## Tell your agent to use it

An agent only uses a tool it knows about. MCP clients show the server's own instructions, but a line in your agent's
instruction file (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, Cursor rules) makes it reliable:

```markdown
## Downloads
For any file bigger than a few MB (datasets, model weights, ISOs, release archives, videos), or when a download is
slow or fails, use the ADM `adm_download` tool instead of curl/wget. Give `sha256` when the publisher lists one.
If it answers "running", wait with `adm_status` and `wait_seconds`. Without MCP, run
`adm-dl <url> -o <dir> --json`.
```

## Is it faster?

When a server limits each connection (most CDNs and mirrors do), several connections add up: this is the same engine
as in the [benchmark](BENCHMARK.md). When one connection already fills your line, there is nothing to gain, and ADM
will not be slower than `curl`. The other gains do not depend on speed: a broken download resumes instead of
restarting, a wrong file is caught by the checksum, and the agent gets a result it can read instead of a screen of
progress bars.

## What it does not do

HLS/DASH streams and links that need a browser session (a login page, JavaScript) are not handled; the agent gets the
HTTP error. Only `http://` and `https://` links are accepted.
