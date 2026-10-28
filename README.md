<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="ADM logo" width="96" height="96">
</p>

<h1 align="center">ADM</h1>

<p align="center">
  A fast, lightweight download manager with multi-connection speed, safe resume and browser integration.
</p>

<p align="center">
  <a href="https://github.com/Axenith/ADM/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Axenith/ADM?color=brightgreen&label=release&sort=semver"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue"></a>
  <img alt="Platforms: Windows and Linux" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-informational">
  <img alt="Built with Rust and Tauri" src="https://img.shields.io/badge/built%20with-Rust%20%2B%20Tauri-orange">
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/main-dark.png">
    <img alt="ADM main window" src="docs/screenshots/main-light.png" width="760">
  </picture>
</p>
<p align="center"><sub>Sample data: the finished and paused rows are made up; the failed row is a real download whose link answered 404.</sub></p>

ADM splits each download across several connections, keeps going when the connection drops, and continues where it stopped. It is a single native program of about 5 MB that uses the system web view instead of shipping its own runtime. A small browser extension sends your browser's downloads to it.

It is inspired by [XDM (Xtreme Download Manager)](https://github.com/subhra74/xdm), which bundles a Java runtime and grew to well over 100 MB. ADM aims at the same behaviour in a fraction of the size.

| | ADM | XDM 9.0.8 |
|---|---|---|
| Installed size | 4.9 MiB, one program | 135 MiB, bundles a JDK |
| 1 GB file, 8 connections | 131 s | 152 s |
| Engine CPU time | 2.6 s | 3.9 s |
| Engine memory | 13 MB | about 98 MB |

One LAN server limiting each connection to about 2 MB/s, two runs each, output byte-identical. Treat it as indicative. The method and caveats are in [docs/BENCHMARK.md](docs/BENCHMARK.md).

## Install

### Windows

1. Download `adm-<version>-windows-x64-setup.exe` from [Releases](https://github.com/Axenith/ADM/releases/latest) and run it. It installs for your user only and needs no administrator rights. It uses the WebView2 runtime that Windows 10 and 11 already include.
2. The installer is not code-signed yet, so Windows SmartScreen may show "Windows protected your PC". Choose **More info**, then **Run anyway**.
3. Install the [browser extension](#browser-extension).

A portable `adm-<version>-windows-x64-portable.zip` is also published. It does not register with browsers.

The Windows build is made by automated tests and CI. It has had less real-world use than the Linux build; please report problems.

### Linux

1. Install WebKitGTK 4.1: `pacman -S webkit2gtk-4.1` (Arch), `apt install libwebkit2gtk-4.1-0` (Debian, Ubuntu) or `dnf install webkit2gtk4.1` (Fedora).
2. Download `adm-<version>-linux-x86_64.tar.gz` from [Releases](https://github.com/Axenith/ADM/releases/latest), then:
   ```sh
   tar xzf adm-*-linux-x86_64.tar.gz
   cd adm-*-linux-x86_64
   ./install.sh            # for your user only; ./install.sh --uninstall removes it
   ```
3. Search for ADM in your application launcher.

Optional: a file manager that supports the `FileManager1` D-Bus interface (Dolphin, Nautilus, Thunar) for Show in folder, `kdialog` or `zenity` for the Browse button, and `gio` for Move to trash.

### Browser extension

For Chrome, Edge, Brave and other Chromium browsers.

1. Download `adm-extension-<version>.zip` from [Releases](https://github.com/Axenith/ADM/releases/latest) and unzip it.
2. Open `chrome://extensions` (or `brave://extensions`), turn on **Developer mode**, choose **Load unpacked**, and select the unzipped folder.
3. Start ADM once, so the browser learns where it is. After that the extension starts ADM by itself when it is needed.

Turn off other download-manager extensions while this one is on, because only one can take over a browser's downloads.

## Features

**Downloading**
- Several connections per file. When one finishes, the busiest range is split, so every connection stays busy to the end.
- Resume that is safe: progress is saved only once it is on disk, and a file that changed on the server is detected instead of being mixed with the old one.
- Expired links: if a signed link is refused after part of the file arrived, ADM asks the original link for a fresh one.
- A queue with a limit on simultaneous downloads, a speed limit, retries with back-off, and servers without range support.
- A finished file never overwrites another: it becomes `name (1).ext`.

**The app**
- A sidebar with Library (All Downloads, Incomplete, Completed) and File types, with live counts; a toolbar with New, Clear, Sort, Settings and search.
- A New download window with file name, category, folder, free space and connections. Several links arrive in one window.
- Pause, resume, retry, open, show in folder, remove, or move to the trash.
- Runs from the tray: closing the window frees its memory while downloads continue. Only one copy runs at a time.
- Dark and light themes.

**Browser extension**
- Browser downloads open in ADM. If ADM is not running, the extension starts it. If that is not possible, the browser downloads as usual, so a download is never lost.
- You choose which file types and sizes it takes over, in ADM's settings.
- Browser monitoring (optional) lists the videos and audio a page loads. With it on, a download is repeated with the headers and cookies the browser really sent.
- Grab links (Alt+Shift+G) collects a page's links or everything it loads, with filters by type, words, wildcard or regular expression.
- Right-click a link and choose Download with ADM. Private windows are never captured.

**For AI agents**
- `adm-dl mcp` is a Model Context Protocol server for Claude Code, opencode, Gemini CLI, Codex and Cursor. `adm-dl <url> --json` serves any agent with a shell. See [docs/AGENTS.md](docs/AGENTS.md).

<p align="center">
  <img alt="New download window" src="docs/screenshots/new-download.png" width="49%">
  <img alt="Batch download window" src="docs/screenshots/batch.png" width="49%">
</p>
<p align="center">
  <img alt="Extension popup listing the videos on a page" src="docs/screenshots/extension-popup.png" height="300">
  &nbsp;
  <img alt="Grab links page" src="docs/screenshots/extension-grab.png" height="300">
</p>

## Privacy and security

- The extension and the app talk over `127.0.0.1` only. ADM answers only requests that name the right host and come from a browser extension, so a web page cannot reach it, and anything that changes state must be a POST.
- Nothing is sent to any server on the internet by the extension or the app. Details: [PRIVACY.md](PRIVACY.md).
- Login cookies and browser monitoring are optional and off until you allow them. The extension's `nativeMessaging` permission can start one program, ADM itself.
- ADM's list, which can hold cookies for links that are waiting, is readable only by you.

## Build from source

You need Rust 1.85 or newer and, on Linux, the [Tauri prerequisites](https://tauri.app/start/prerequisites/#linux) (WebKitGTK 4.1, an appindicator library, librsvg, a C toolchain).

```sh
cargo build --release -p adm          # the app
cargo build --release -p adm-cli      # adm-dl: command line and MCP server
./packaging/install-local.sh          # Linux: install the build for your user
packaging/release.sh                  # Linux: tarball and extension zip into dist/
```

The Windows installer is built by [`.github/workflows/windows.yml`](.github/workflows/windows.yml).

## Documentation

- [Architecture](docs/ARCHITECTURE.md): how the engine, the app and the extension fit together, and how each part maps to XDM.
- [ADM for AI agents](docs/AGENTS.md)
- [Benchmark](docs/BENCHMARK.md)
- [Release notes](docs/releases)

## Development

```sh
cargo test -p adm-core -p adm                    # engine (against a real local HTTP server) and app logic
node --test extension/test/basic.test.mjs extension/test/enhanced.test.mjs extension/test/media.test.mjs
node extension/test/browser-e2e.mjs              # the extension in a real headless Brave or Chromium
python3 tests/ui/run.py                          # the interface, clicked in a headless browser
python3 tests/e2e/e2e.py                         # also e2e2 to e2e5 and agent_mcp.py: the real app on Linux
```

The end-to-end tests run their own copy of ADM on a separate port and data folder, so they never touch a running ADM.

```
crates/adm-core/   download engine (library)
crates/adm-cli/    adm-dl: command line, MCP server, browser launcher
src-tauri/         the app: window, tray, queue, dialogs, local server
ui/                the interface (HTML, CSS, JavaScript; no build step)
extension/         the browser extension (Manifest V3, no build step)
packaging/         installers and release scripts
tests/             end-to-end and interface tests
```

## Roadmap

- HLS and DASH streams (most video sites), with merging of audio and video.
- Chrome Web Store listing and a Firefox extension.
- macOS build, code signing, `.deb` and `.rpm` packages.
- Scheduler, proxy authentication.

## License

ADM is licensed under the [Apache License 2.0](LICENSE); see [NOTICE](NOTICE). It is inspired by [XDM](https://github.com/subhra74/xdm) by Subhra Das Gupta. ADM's code is written from scratch and follows the same well-known techniques: range requests, splitting the busiest range, and `If-Range` resume safety.
