"""Which browser headers reach the origin server, and what happens when the port is taken by another program."""
import socket, subprocess, threading, time
from common import *

seed_db(False)
start_hdr_server()

print("== A: browser headers on the wire")
app = start_app()
st, _ = post_download({"url": f"http://127.0.0.1:{WEB_PORT}/files/hdr.bin", "filename": "adm-e2e-b.bin",
    "requestHeaders": {"If-None-Match": ["zzz"], "Accept-Encoding": ["gzip, br"], "X-Custom": ["kept"], "Authorization": ["Bearer tok"], "Range": ["bytes=999-"]}})
check("download accepted", st == 200)
p = wait_file("adm-e2e-b.bin"); logs = hdr_log()
check("download completed", p is not None, f"({len(logs)} requests)")
check("If-None-Match NOT replayed (would give 304)", logs and all(l["inm"] is None for l in logs))
check("browser's Accept-Encoding NOT replayed (engine asks for identity)", logs and all(l["ae"] == "identity" for l in logs))
check("browser's Range NOT replayed (engine sets its own)", logs and not any(l["range"] and l["range"].startswith("bytes=999") for l in logs))
check("custom header X-Custom replayed", logs and all(l["custom"] == "kept" for l in logs))
check("Authorization replayed (needed for protected downloads)", logs and all(l["auth"] == "Bearer tok" for l in logs))
check("tray Quit works", tray_quit(app.pid) and wait_dead(app.pid))

print("== B: the port is held by something that is not ADM")
hits = []
sq = socket.socket(); sq.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); sq.bind(("127.0.0.1", PORT)); sq.listen(5)
def squat():
    while True:
        try: c, _ = sq.accept()
        except OSError: return
        d = c.recv(4096).decode(errors="replace"); hits.append(d.split("\r\n")[0])
        body = b'{"enabled":true,"instanceId":"other-program","fileExts":[]}'
        c.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + str(len(body)).encode() + b"\r\nConnection: close\r\n\r\n" + body); c.close()
threading.Thread(target=squat, daemon=True).start()
app2 = subprocess.Popen(["nice", "-n", "10", APP], stdout=open(f"{TMP}/app2.log", "w"), stderr=subprocess.STDOUT, start_new_session=True)
time.sleep(5)
check("ADM keeps running (does not exit or crash)", alive(app2.pid) and app2.poll() is None)
check("it only probed (GET /sync); it never POSTed /show to the other program", hits and all(h.startswith("GET /sync") for h in hits), str(hits))
check("it told the user why the extension is off", "browser integration is off" in open(f"{TMP}/app2.log").read())
check("tray Quit works without the server", tray_quit(app2.pid) and wait_dead(app2.pid))
sq.close()
print("\nALL PASS" if ok else "\nSOME FAILED")
raise SystemExit(0 if ok else 1)
