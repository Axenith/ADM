"""Takes the README's main-window screenshot from the real app: `python3 tests/e2e/shots_main.py dark|light out.png`.
The finished and paused rows are made-up sample data; the failed row is a real download whose link answers 404, and the
two running ones are real (a slow local server). Needs a desktop session with hyprctl and grim."""
import json, os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
theme, out = sys.argv[1], sys.argv[2]
os.environ["HDR_SIZE"] = str(32 * 1024 * 1024)
if theme == "light": os.environ["GTK_THEME"] = "Adwaita:light"
from common import *
seed_db(False, max_simultaneous=1)
d = read_db(); dl = DL
def rec(i, name, total, done, status, error=None):
    return {"id": 9000000000000 + i, "url": "https://example.org/files/" + name.replace(" ", "%20"), "dir": dl, "name": name, "total": total, "downloaded": done,
            "status": status, "error": error, "path": f"{dl}/{name}" if status == "completed" else None, "headers": [], "cookie": None, "file_name": None}
d["records"] = [rec(1, "ubuntu-24.04.3-desktop-amd64.iso", 6_200_000_000, 6_200_000_000, "completed"),
                rec(2, "Lecture 07 - Transformers.pdf", 4_300_000, 4_300_000, "completed"),
                rec(3, "holiday-clips-2026.zip", 882_000_000, 312_000_000, "paused"),
                rec(5, "Interstellar (2014) 1080p.mkv", 3_400_000_000, 3_400_000_000, "completed"),
                rec(6, "podcast-episode-212.mp3", 58_000_000, 58_000_000, "completed"),
                rec(7, "VSCode-1.96-linux-x64.deb", 98_000_000, 98_000_000, "completed"),
                rec(8, "dataset-images.tar.gz", 1_900_000_000, 1_900_000_000, "completed")]
open(DB, "w").write(json.dumps(d))
start_hdr_server(); app = start_app()
post_download({"url": f"http://127.0.0.1:{WEB_PORT}/gone/setup-installer.exe", "filename": "setup-installer.exe"})  # a real 404
time.sleep(1.5)
for name in ("Big Buck Bunny 1080p.mp4", "Sintel Trailer 4K.mp4"):
    post_download({"url": f"http://127.0.0.1:{WEB_PORT}/slow/{name.replace(' ', '-')}", "filename": name, "fileSize": 32 * 1024 * 1024})
    time.sleep(0.5)
time.sleep(9)
clients = json.loads(subprocess.run(["hyprctl", "clients", "-j"], capture_output=True, text=True).stdout)
mine = [c for c in clients if c["pid"] == app.pid]
x, y = mine[0]["at"]; w, h = mine[0]["size"]
subprocess.run(["grim", "-g", f"{x},{y} {w}x{h}", out]); print("saved", out, w, h)
tray_quit(app.pid); wait_dead(app.pid)
