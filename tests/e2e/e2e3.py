"""The "New download" dialog in the real app: a captured link waits for the user instead of starting; the window
asks the server about it; it survives closing ADM; the blocked-sites list reaches the extension.
Set E2E_SHOT=/some/file.png to also save a screenshot of the window with the dialog showing."""
import json, os, subprocess, time
from common import *

PAGE = "https://site.example/watch?v=1"
REMEMBERED = f"{TMP}/Remembered by ADM"   # a folder chosen in an earlier session
os.makedirs(REMEMBERED)
seed_db(True, blocked=["blocked.example"], last_folder=REMEMBERED)
start_hdr_server()
app = start_app()

print("== a captured link waits in the dialog")
s, _ = post_download({"url": f"http://127.0.0.1:{WEB_PORT}/files/pending.bin", "cookie": "sid=secret1", "filename": "pending.bin",
                      "fileSize": SIZE, "referer": PAGE})
check("POST /download accepted", s == 200)
time.sleep(5)  # the window opens and asks the server about the link
logs = hdr_log()
check("the download did NOT start (no file, no part file)", not os.listdir(DL), str(os.listdir(DL)))
check("the dialog asked the server about it (one-byte range request)", any(l["range"] == "bytes=0-0" for l in logs), f"{len(logs)} request(s)")
check("...with the browser's cookie", any(l["cookie"] == "sid=secret1" for l in logs))
check("...and nothing beyond that first byte was fetched", all(l["range"] == "bytes=0-0" for l in logs))
db = read_db()
check("the link is kept in the list, waiting", len(db["pending"]) == 1 and db["records"] == [])
p = db["pending"][0]
check("it remembers name, size and the page it came from", (p["file_name"], p["size"], p["page"]) == ("pending.bin", SIZE, PAGE), str((p["file_name"], p["size"], p["page"])))
check("the settings file is readable only by its owner", oct(os.stat(DB).st_mode & 0o777) == "0o600", oct(os.stat(DB).st_mode & 0o777))

print("== the don't-capture list reaches the extension")
s, t = req("GET", "/sync", headers={"Origin": "chrome-extension://abcdef"}); hosts = json.loads(t)["blockedHosts"]
check("the user's blocked site is in /sync", "blocked.example" in hosts, str(hosts))
check("...next to ADM's built-in ones", "update.microsoft.com" in hosts)

if os.environ.get("E2E_SHOT"):
    clients = json.loads(subprocess.run(["hyprctl", "clients", "-j"], capture_output=True, text=True).stdout)
    mine = [c for c in clients if c["pid"] == app.pid]
    if mine:
        x, y = mine[0]["at"]; w, h = mine[0]["size"]
        subprocess.run(["grim", "-g", f"{x},{y} {w}x{h}", os.environ["E2E_SHOT"]]); print("  info  screenshot:", os.environ["E2E_SHOT"])

print("== closing ADM does not lose it")
check("tray Quit works", tray_quit(app.pid) and wait_dead(app.pid))
check("the waiting link is still saved", len(read_db()["pending"]) == 1)
before = len(hdr_log())
app = start_app()
time.sleep(5)
check("after restarting, the dialog comes back and asks the server again", len(hdr_log()) > before, f"{before} -> {len(hdr_log())} requests")
check("still nothing downloaded", not os.listdir(DL))
check("tray Quit works again", tray_quit(app.pid) and wait_dead(app.pid))
print("\nALL PASS" if ok else "\nSOME FAILED")
raise SystemExit(0 if ok else 1)
