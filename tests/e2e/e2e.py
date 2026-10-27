"""Attacks ADM's loopback server like a web page would, then runs a legitimate extension download and the
second-launch hand-off. The dialog is off here: these checks are about starting downloads."""
import subprocess, time
import subprocess as _sp
from common import *

seed_db(False)
start_hdr_server()
app = start_app()

print("== server basics")
s, t = req("GET", "/sync"); j = json.loads(t)
check("GET /sync -> 200 with ADM config", s == 200 and j["app"] == "adm" and "ZIP" in j["fileExts"] and j["instanceId"])
s, _ = req("GET", "/sync", headers={"Origin": "chrome-extension://abcdef"}); check("extension origin accepted", s == 200)
print("== attacks from a web page")
s, _ = req("GET", "/sync", headers={"Origin": "https://evil.example"}); check("page Origin on /sync refused (403)", s == 403, f"got {s}")
s, _ = post_download({"url": f"http://127.0.0.1:{WEB_PORT}/evil.bin"}, "https://evil.example"); check("POST /download from page refused (403)", s == 403, f"got {s}")
s, _ = post_download({"url": f"http://127.0.0.1:{WEB_PORT}/evil2.bin"}, "null"); check("sandboxed-frame Origin 'null' refused (403)", s == 403, f"got {s}")
s, _ = req("GET", "/sync", host="evil.example:8598"); check("DNS-rebinding Host refused (403)", s == 403, f"got {s}")
s, _ = req("GET", "/download?url=x"); check("simple GET /download refused (405)", s == 405, f"got {s}")
s, _ = req("GET", "/show"); check("simple GET /show refused (405)", s == 405, f"got {s}")
s, _ = req("GET", "/nope"); check("unknown path 404", s == 404, f"got {s}")
time.sleep(1.5)
check("none of the refused requests started a download", not any(os.path.exists(os.path.join(DL, n)) for n in ("evil.bin", "evil2.bin")) and not hdr_log())

print("== legitimate extension download")
msg = {"url": f"http://127.0.0.1:{WEB_PORT}/files/e2e-test.bin", "cookie": "session=abc123", "filename": "sub/adm-e2e.bin", "fileSize": SIZE,
       "requestHeaders": {"Referer": ["https://site.example/page"], "Range": ["bytes=0-"]}}
s, t = post_download(msg); check("POST /download accepted (200 + config)", s == 200 and "adm" in t)
p = wait_file("adm-e2e.bin"); check("file downloaded under the browser's chosen name (sub/ stripped)", p is not None)
check("content is byte-exact", p is not None and exact(p))
logs = hdr_log()
check("origin server saw the browser's cookie", any(l["cookie"] == "session=abc123" for l in logs), f"({len(logs)} requests)")
check("origin server saw the browser's Referer", any(l["referer"] == "https://site.example/page" for l in logs))
check("used several connections (ranges)", len(logs) > 2)

print("== second launch hands over to the first")
t0 = time.time(); r = subprocess.run([APP, f"http://127.0.0.1:{WEB_PORT}/files/second.bin"], timeout=15); dt = time.time() - t0
check("second launch exits by itself", r.returncode == 0, f"(rc={r.returncode}, {dt:.1f}s)")
p2 = wait_file("second.bin"); check("link passed to the first instance was downloaded", p2 is not None and exact(p2))

check("tray Quit works", tray_quit(app.pid) and wait_dead(app.pid))
print("\nALL PASS" if ok else "\nSOME FAILED")
raise SystemExit(0 if ok else 1)
