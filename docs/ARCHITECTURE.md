# ADM: architecture

A small, fast, open-source download manager in Rust (engine) and Tauri (UI), modelled on
[XDM](https://github.com/subhra74/xdm). This document records what XDM does, how each part maps to
ADM, and what is built so far.

## Why not just use XDM?

XDM 8+ is a Kotlin/Swing application (~45k lines) that ships its own JDK, which is where the install size
comes from. The download logic itself is modest; the weight is the runtime. ADM keeps the same
behaviour and drops the runtime: the engine compiles to a ~2.8 MB native binary (HTTP + TLS included) and
the UI uses the system webview through Tauri instead of bundling a browser engine.

## XDM, as read from its source

| XDM module (Kotlin) | What it does | ADM |
|---|---|---|
| `downloaders/web/http/HttpDownloader`, `HttpChunkRetriever` | Segmented HTTP download. First request is `Range: bytes=0-` and doubles as the probe. Connections are kept busy by splitting the chunk with the most bytes left (needs >= 256 KiB left) and restarting failed chunks when a slot frees. | `coord.rs`, `segment.rs` (done) |
| `network/http/impl/HttpClientImpl` (OkHttp) | HTTP/1.1 only (so each segment is its own connection), proxy auth (Basic, NTLM), shared TLS context, optional "ignore cert errors". | `engine.rs` on reqwest + rustls/ring. Proxy: done. NTLM/proxy-auth prompts: later |
| `downloaders/web/StateSaver`, `TaskInfoDB`, `AtomicIO` | Binary state per download, written atomically; only *fsynced* progress is recorded, so a crash re-fetches unsynced bytes. Sensitive headers are stripped. | `store.rs` (done, JSON) |
| `downloaders/web/SpeedLimiter`, `ProgressTracker` | Sleep-based limiter; smoothed speed over 6 samples; ETA. | `limiter.rs`, `progress.rs` (done) |
| `downloaders/web/streaming/*` | HLS (`HlsParser`, `HlsDownloaderTask`) and DASH (`manifest/dash/*`): parse manifest, download segments in parallel, AES-128 decrypt, merge. | Phase 3 |
| `media/muxer/*` (~13k lines incl. tests) | Pure-JVM transmuxer (TS/MP4/MKV) plus an `FFmpegMuxer` fallback. | Phase 3: use `ffmpeg` if installed; native muxing only if worth the size |
| `downloaders/web/batch/*` | Many links as one entry. | Phase 4 |
| `integration/BrowserIntegration` | Loopback HTTP server on `127.0.0.1:8597` (also the single-instance lock). Endpoints `/download /batch /media /vid /clear /clear-tab /tab-update /show /poll /sync`. Rejects non-extension `Origin`, checks `Host`, POST-only for state changes (DNS-rebinding defence). | Phase 2 |
| `integration/VideoHelper`, `CapturedVideoTracker` | Detects videos from the extension's media messages. | Phase 3 |
| `app/DownloadManager`, `DownloadsDB`, `DuplicateIndex`, `QueueService`, `DownloadScheduler`, `CategoryRules` | Queues, scheduling, categories, duplicate detection, download list. | Phase 2 (app crate) |
| `app/ui/*` (~13k lines Swing) | The window. | Tauri + a small web UI, Phase 2 |
| `browser-extension/` | Chrome/Firefox extension (GPLv3). | Phase 2: reuse as is, or write a compatible one |

## Engine design (`adm-core`)

One **coordinator task** per download owns every decision; **segment tasks** only move bytes. XDM
solves the same problem with threads and a read/write lock around a shared map; here the coordinator is
the only place the segment set changes, so there is nothing to lock around.

```
Engine::start(req) ──► coordinator task ──► segment task ×N  (one TCP connection each)
                          ▲   │ events: Init · Progress · Paused · Completed · Failed
                          └───┘ SegMsg: Finished / Failed / Stopped
```

**Concurrency contract (the part worth getting right).** A segment is `[offset, offset+length)`. The only
shared field is its small `SegState` mutex. Before writing, a segment *reserves* bytes
(`reserved += n`, clamped to `length`). A split cuts `length` by half of `length - reserved` under the
same lock, so it can never take back bytes a segment already claimed. Bytes beyond the new end that were
already in flight are dropped and fetched again by the new segment. No byte is ever written by two
segments, and no byte is missed.

**Flow**
1. *Probe.* `Range: bytes=0-`. `206` + `Content-Range` ⇒ size known, ranges work. `200` ⇒ single stream
   (size from `Content-Length`, or unknown). `416` ⇒ empty file, ask again without a range.
2. *Preallocate* the part file (`.<id>.admpart`, in the destination folder so the final rename is atomic),
   hand the probe's live response to segment 0, then split until `max_segments` are running.
3. *Run.* A freed connection restarts a failed/waiting segment, else halves the busiest one. Writes are
   positional (`pwrite`) on one shared file handle, 256 KiB at a time, flushed at least every 250 ms.
4. *Persist.* `fsync` after 64 MiB or 30 s, then save state. Only synced bytes count as progress.
5. *Finish.* Verify segments tile `0..size` exactly, `fsync`, claim a free file name with `create_new`
   (`name (1).ext` if taken, never overwrites), rename, delete state.

**Resume safety** (ported from XDM, with reasons)
- `If-Range: <Last-Modified | strong ETag>` on bounded requests. A changed file answers `200`, which is
  refused (`ResumeNotSupported`) instead of splicing two versions. `Last-Modified` is preferred because
  mirrors of one file share a date but not an ETag.
- A `206` must report the start offset that was asked for.
- Redirect targets are cached for speed; if one starts returning 401/403/410 the engine goes back
  through the original link (signed URLs expire). After bytes are on disk, a refused original link is
  `LinkExpired` so the UI can offer "refresh link".
- A missing or wrong-sized part file is a `Disk` error. The engine never silently starts over on top of
  user data. It starts over only when nothing could be continued anyway (no size, or no range support).

**Deliberate differences from XDM**
- No "takeover" of unstarted neighbour chunks; splitting from the start gives the same balance.
- Exponential retry backoff (1, 2, 4 … 30 s) instead of a flat 5 s; a retry that made progress is free.
- State is JSON (versioned) rather than a hand-rolled binary format.
- `Accept-Encoding: identity`, HTTP/1.1 only, rustls with `ring` (no `aws-lc`, no `h2`) to keep it small.

## Status

| | |
|---|---|
| ✅ Phase 1 | `adm-core` HTTP engine + `adm` CLI. 7 unit + 12 end-to-end tests (byte-exact segmented download, no-range server, unknown length, dropped connections, pause/resume, speed limit, naming, no-overwrite, errors). |
| ✅ Phase 2 (core) | Tauri app (4.9 MiB binary): download list, pause/resume/retry/remove, settings, persistence, tray mode (closing the window frees the webview), `adm <link>` launch, single-instance lock, and a loopback server on `127.0.0.1:8598` (its own port, so it runs alongside XDM) speaking an XDM-style protocol (`/sync /download /poll /show`, Host/Origin guards, POST-only state changes). Cookie, Referer, custom headers and the browser's file name are replayed. Verified end to end (`tests/e2e`). |
| ✅ New download window | A link from the browser or the link box waits in a dialog (name, category, folder with a native Browse button, free space, connections, "Don't capture from this site") before it starts; several links queue; waiting links are saved across restarts. `Engine::probe` asks the server for name/size/resumability with a one-byte request. Blocked sites are published to the extension through `/sync`. Verified in the real app (`tests/e2e/e2e3.py`) and with 41 browser-driven UI checks (`tests/ui/run.py`). |
| ✅ Queue and batches | At most `max_simultaneous` downloads run (default 3, 0 = no limit); the rest wait as "queued" and start in order as others finish. A batch of links ("Grab all links", `POST /batch`, up to 200) waits in one window and starts through the same queue. Checked in the real app (`tests/e2e/e2e4.py`: never more than one at a time, FIFO). |
| ✅ Browser extension | `extension/`: Manifest V3, hands browser downloads and "Download with ADM" links to ADM; never loses a download if ADM is off. v0.3.0 adds browser monitoring (video list), "Grab all links", an options page and an XDM-style popup. Tested with a mocked browser (29 tests), against the real app, and in a REAL headless Brave (`extension/test/browser-e2e.mjs`). |
| ⏳ Phase 2 (left) | New-download confirmation dialog, queue/scheduler, categories, duplicate detection, notifications, update to Tauri 2.12.5+ once `tray-icon` 0.25 is on crates.io. |
| ⏳ Phase 3 | HLS / DASH (m3u8 parsing, AES-128, merge via ffmpeg), video detection from the extension. |
| ⏳ Phase 4 | Batch downloads, scheduler, categories, duplicate detection, proxy/server auth prompts, checksum verification. |

## Licensing

XDM's engine (`xdm-core`) is **Apache-2.0**; its app and browser extension are **GPLv3**. ADM's engine
is a fresh implementation of the same well-known techniques (HTTP range requests, work splitting), not a
line-by-line translation, so it is free of GPL obligations. Anything copied from `xdm-app` or the
extension (UI code, extension JS) would make that part GPLv3. Pick ADM's license before publishing;
if `xdm-core` code is ever ported directly, keep its NOTICE.
