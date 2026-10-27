"""The download queue and batch links in the real app.
  1. dialog off, ONE download at a time, three links in one batch: one runs, two wait, they start in order, and
     never more than one runs at once.
  2. dialog on: a batch waits in ONE dialog (not one per link); it survives a restart.
Set E2E_SHOT=/some/file.png to save a screenshot of the batch dialog."""
import json, os, subprocess, time
from common import *

start_hdr_server()
names = ["slow1.bin", "slow2.bin", "slow3.bin"]
batch = {"items": [{"url": f"http://127.0.0.1:{WEB_PORT}/slow/{n}", "filename": n, "fileSize": SIZE} for n in names],
         "referer": "https://site.example/gallery", "userAgent": "E2E/1.0"}

print("== 1. the queue: one at a time")
seed_db(False, max_simultaneous=1)
app = start_app()
s, _ = req("POST", "/batch", batch, {"Origin": "chrome-extension://abcdef"}); check("POST /batch accepted", s == 200)
first, started, max_running, snapshots = None, [], 0, []
t0 = time.time()
while time.time() - t0 < 40:
    recs = {r["name"]: r["status"] for r in read_db()["records"]}
    if recs and not snapshots: first = dict(recs)
    snapshots.append(recs)
    max_running = max(max_running, sum(1 for v in recs.values() if v == "downloading"))
    for n in names:
        if recs.get(n) == "downloading" and n not in started: started.append(n)
    if len(recs) == 3 and all(v == "completed" for v in recs.values()): break
    time.sleep(0.1)
check("all three links were added", len(first or {}) >= 1 and len(snapshots[-1]) == 3, str(snapshots[-1] if snapshots else None))
saw_queue = any(sum(1 for v in s.values() if v == "queued") == 2 and sum(1 for v in s.values() if v == "downloading") == 1 for s in snapshots)
check("at first one runs and two wait in the queue", saw_queue)
check("never more than one download at the same time", max_running == 1, f"max running = {max_running}")
check("they start in the order they were sent", started == names, str(started))
check("all three completed", all(v == "completed" for v in snapshots[-1].values()), str(snapshots[-1]))
check("every file is byte-exact", all((p := os.path.join(DL, n)) and os.path.exists(p) and exact(p) for n in names))
check("the browser's referer reached the server", any(l["referer"] == "https://site.example/gallery" for l in hdr_log()))
check("tray Quit works", tray_quit(app.pid) and wait_dead(app.pid))

print("== 2. a batch waits in one dialog")
for n in names: os.remove(os.path.join(DL, n))
open(HDR_LOG, "w").close()
seed_db(True)
app = start_app()
s, _ = req("POST", "/batch", batch, {"Origin": "chrome-extension://abcdef"}); check("POST /batch accepted", s == 200)
time.sleep(4)
db = read_db()
check("ONE waiting entry holds all three links (not three dialogs)", len(db["pending"]) == 1 and len(db["pending"][0]["batch"]) == 3, str(len(db["pending"])))
check("...with the names the page gave", [i["name"] for i in db["pending"][0]["batch"]] == names)
check("nothing started, nothing fetched", db["records"] == [] and not os.listdir(DL) and not hdr_log())
if os.environ.get("E2E_SHOT"):
    clients = json.loads(subprocess.run(["hyprctl", "clients", "-j"], capture_output=True, text=True).stdout)
    mine = [c for c in clients if c["pid"] == app.pid]
    if mine:
        x, y = mine[0]["at"]; w, h = mine[0]["size"]
        subprocess.run(["grim", "-g", f"{x},{y} {w}x{h}", os.environ["E2E_SHOT"]]); print("  info  screenshot:", os.environ["E2E_SHOT"])
check("tray Quit works", tray_quit(app.pid) and wait_dead(app.pid))
check("the group is still saved after closing ADM", len(read_db()["pending"]) == 1 and len(read_db()["pending"][0]["batch"]) == 3)
app = start_app(); time.sleep(3)
check("after a restart it comes back as one group", len(read_db()["pending"]) == 1)
check("tray Quit works again", tray_quit(app.pid) and wait_dead(app.pid))
print("\nALL PASS" if ok else "\nSOME FAILED")
raise SystemExit(0 if ok else 1)
