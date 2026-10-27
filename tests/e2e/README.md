End-to-end checks of the real desktop app (Linux; needs `target/release/adm`, `busctl`, and for the screenshot option
`hyprctl` + `grim`).

Every run is **isolated from a running ADM**: it uses its own port (`ADM_PORT`), its own data folder
(`ADM_DATA_DIR`: the download list and settings) and its own download folder, all in one temporary directory that is
removed at the end. A real ADM and its list are never touched. `common.py` sets this up.

    python3 tests/e2e/e2e.py     # attacks the server like a web page; a legitimate download; second launch hands over
    python3 tests/e2e/e2e2.py    # which browser headers reach the origin; the port being taken by another program
    python3 tests/e2e/e2e3.py    # the New download window: a link waits, is probed, survives a restart; blocked sites
                                 # (E2E_SHOT=out.png saves a screenshot of the real window with the dialog showing)

    python3 tests/e2e/e2e4.py    # the download queue (one at a time, in order) and a batch of links in ONE window

`hdr_server.py` is a small range-capable file server that logs the headers it receives. The scripts open an ADM window
on your screen for a few seconds.
