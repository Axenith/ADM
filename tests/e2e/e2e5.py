"""The browser extension starts ADM: the native messaging host (ADM itself, started by the browser with the
extension's origin), the registration of that host with the browsers, and the quiet start in the tray.
Isolated like the other e2e scripts (own port, data and config folders)."""
import json, os, re, struct, subprocess, sys, time
from common import *
from common import _procs

ORIGIN = "chrome-extension://hncgajannlpkmglkanljjngglcjhbdnm/"
STORE_ORIGIN = "chrome-extension://lbgnfakfhkaicndhpdpfdeaceokhmnae/"

def host(origin, message, timeout=15):
    """What a browser does: starts the program with the origin, sends one length-prefixed JSON message, reads the reply."""
    p = subprocess.Popen([APP, origin], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    body = json.dumps(message).encode()
    out, _ = p.communicate(struct.pack("<I", len(body)) + body, timeout=timeout)
    n = struct.unpack("<I", out[:4])[0]
    return json.loads(out[4:4 + n]), len(out) == 4 + n, p.returncode

def listener_pid():
    out = subprocess.run(["ss", "-ltnpH", f"sport = :{PORT}"], capture_output=True, text=True).stdout
    m = re.search(r"pid=(\d+)", out); return int(m.group(1)) if m else None

def windows_of(pid):
    cl = json.loads(subprocess.run(["hyprctl", "clients", "-j"], capture_output=True, text=True).stdout)
    return [c for c in cl if c["pid"] == pid]

# --- registration with the browsers --------------------------------------------------------------------------
cfg = f"{TMP}/cfg"
for b in ("google-chrome", "BraveSoftware/Brave-Browser"): os.makedirs(f"{cfg}/{b}")
env = dict(os.environ, XDG_CONFIG_HOME=cfg)
r = subprocess.run([APP, "--register-native-host"], env=env, capture_output=True, text=True, timeout=20)
files = [f"{cfg}/google-chrome/NativeMessagingHosts/dev.adm.launcher.json", f"{cfg}/BraveSoftware/Brave-Browser/NativeMessagingHosts/dev.adm.launcher.json"]
check("register: writes the host file for each browser that is installed", all(os.path.exists(f) for f in files), r.stdout)
check("register: leaves browsers that are not installed alone", not os.path.exists(f"{cfg}/chromium"))
m = json.load(open(files[0]))
check("register: points at this program, as stdio", m["name"] == "dev.adm.launcher" and m["path"] == APP and m["type"] == "stdio", json.dumps(m))
check("register: only the ADM extension (unpacked or from the Web Store) may start it", m["allowed_origins"] == [ORIGIN, STORE_ORIGIN], json.dumps(m["allowed_origins"]))
before = os.stat(files[0]).st_mtime_ns
subprocess.run([APP, "--register-native-host"], env=env, capture_output=True, timeout=20)
check("register: running it again changes nothing", os.stat(files[0]).st_mtime_ns == before)
subprocess.run([APP, "--unregister-native-host"], env=env, capture_output=True, timeout=20)
check("unregister: removes the files", not any(os.path.exists(f) for f in files))


# --- the host starts ADM, in the tray ------------------------------------------------------------------------
start_hdr_server(); seed_db(True)
check("before: ADM is not running", listener_pid() is None)
reply, clean, code = host(ORIGIN, {"op": "start"})
check("host: answers ok, 'not running before', as a clean length-prefixed message", reply == {"ok": True, "running": False} and clean and code == 0, json.dumps(reply))
t = time.time()
while listener_pid() is None and time.time() - t < 20: time.sleep(0.25)
pid = listener_pid()
check("host: ADM is up and answering on its port", pid is not None and req("GET", "/sync")[0] == 200)
if pid: _procs.append(type("P", (), {"pid": pid})())
time.sleep(2)
check("host: it started in the tray, with no window", not windows_of(pid), str([w["class"] for w in windows_of(pid)]))
reply, _, _ = host(ORIGIN, {"op": "start"})
check("host: asked again while ADM runs, it says so and starts nothing", reply == {"ok": True, "running": True} and listener_pid() == pid, json.dumps(reply))
reply, _, _ = host(ORIGIN, {"op": "nope"})
check("host: an unknown request is refused, not guessed at", reply["ok"] is False)
st, _ = req("POST", "/download", {"url": f"http://127.0.0.1:{WEB_PORT}/f.bin", "filename": "f.bin", "fileSize": SIZE}, {"Origin": "chrome-extension://abcdef"})
time.sleep(2.5)
check("a download sent afterwards opens the New download window", st == 200 and len(windows_of(pid)) >= 1, str([w["title"] for w in windows_of(pid)]))
tray_quit(pid); check("ADM quits from the tray", wait_dead(pid))

print("\nALL PASS" if ok else "\nSOME FAILED"); sys.exit(0 if ok else 1)
