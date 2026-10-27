# Benchmark: ADM vs XDM 9.0.8

Date: 2026-10-28. One 1,063,899,866-byte file from a LAN nginx server that caps each connection at about
2 MB/s (one connection: 2.0 MB/s, four: 5.6 MB/s), so parallel connections matter. Both tools used 8
connections, run headless, alternating ADM, XDM, ADM, XDM, at the same CPU priority. Output files
were byte-identical (same SHA-256). Script: `docs/bench-vs-xdm.py URL`.

| Run | Time | Avg speed | CPU time | Peak memory (PSS) |
|---|---|---|---|---|
| ADM #1 | 132.9 s | 7.64 MB/s | 2.6 s | 13 MB |
| ADM #2 | 129.8 s | 7.82 MB/s | 2.6 s | 13 MB |
| XDM #1 | 155.2 s | 6.54 MB/s | 3.9 s | 96 MB |
| XDM #2 | 148.1 s | 6.85 MB/s | 3.9 s | 99 MB |
| **Average** | **ADM 131.4 s / XDM 151.7 s** | **7.73 / 6.69 MB/s** | **2.6 / 3.9 s** | **13 / ~98 MB** |

Caveats: one server, one network, two runs each. XDM's time includes JVM start-up (about a second at most).
Memory is the download engine only (ADM CLI vs XDM headless); the ADM GUI adds a webview (~250 MB total
measured while downloading) and XDM's GUI adds its own. Install size: ADM GUI 4.9 MiB binary, XDM 135 MiB.
