"""Shared set-up for the end-to-end scripts. Every run is isolated from a real ADM: its own port (ADM_PORT), its own
data folder (ADM_DATA_DIR, which holds the download list and settings) and its own download folder, all under one
temporary directory that is removed at the end. A running ADM and its list are never touched."""
import atexit, http.client, json, os, shutil, socket, subprocess, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
APP = os.environ.get("ADM_BIN", f"{ROOT}/target/release/adm")
PORT = int(os.environ.get("E2E_PORT", "18598"))
WEB_PORT = 18090
SIZE = 3 * 1024 * 1024

TMP = tempfile.mkdtemp(prefix="adm-e2e-")
DATA, DL, HDR_LOG = f"{TMP}/data", f"{TMP}/dl", f"{TMP}/hdr.log"
DB = f"{DATA}/downloads.json"
os.makedirs(DATA); os.makedirs(DL); open(HDR_LOG, "w").close()
os.environ.update(ADM_PORT=str(PORT), ADM_DATA_DIR=DATA, HDR_LOG=HDR_LOG)

_procs = []
@atexit.register
def _cleanup():
    for p in _procs:
        try: os.killpg(p.pid, 9)
        except Exception: pass
    shutil.rmtree(TMP, ignore_errors=True)

ok = True
def check(name, cond, extra=""):
    global ok; ok &= bool(cond); print(f"  {'PASS' if cond else 'FAIL'}  {name} {extra}", flush=True)

def byte(i): return (i * 31 + 7) & 0xFF

def seed_db(ask, blocked=(), pending=(), last_folder=None, remember=True, **settings_extra):
    """Writes the app's list as it should be before launch."""
    settings = {"max_connections": 8, "speed_limit_kib": 0, "download_dir": DL, "ask_before_download": ask,
                "sort_by_category": False, "blocked_sites": list(blocked),
                "remember_folder": remember, "last_folder": last_folder, **settings_extra}
    open(DB, "w").write(json.dumps({"settings": settings, "records": [], "pending": list(pending)}))

def read_db(): return json.load(open(DB))

def start_hdr_server():
    p = subprocess.Popen(["python3", "-I", f"{HERE}/hdr_server.py"], stderr=subprocess.DEVNULL, start_new_session=True)
    _procs.append(p)
    for _ in range(40):
        try: socket.create_connection(("127.0.0.1", WEB_PORT), 0.3).close(); return p
        except OSError: time.sleep(0.1)
    raise RuntimeError("file server did not start")

def start_app(*args):
    p = subprocess.Popen(["nice", "-n", "10", APP, *args], stdout=open(f"{TMP}/app.log", "a"), stderr=subprocess.STDOUT, start_new_session=True)
    _procs.append(p)
    for _ in range(80):
        try: socket.create_connection(("127.0.0.1", PORT), 0.3).close(); return p
        except OSError: time.sleep(0.25)
    raise RuntimeError("ADM did not start listening")

def alive(pid):
    try: return open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[0] != "Z"  # a zombie is not alive
    except OSError: return False

def wait_dead(pid, secs=15):
    t = time.time()
    while alive(pid) and time.time() - t < secs: time.sleep(0.25)
    return not alive(pid)

def tray_quit(pid):
    """Clicks "Quit" in the app's tray menu over D-Bus: the same as clicking it, so downloads pause and save."""
    items = subprocess.run(["busctl", "--user", "get-property", "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher",
                            "org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems"], capture_output=True, text=True).stdout.split('"')
    for it in items:
        if f"tray_icon_tray_app_{pid}_" in it:
            name, path = it.split("/", 1)
            subprocess.run(["busctl", "--user", "call", name, "/" + path + "/Menu", "com.canonical.dbusmenu", "Event", "isvu", "3", "clicked", "i", "0", "0"], capture_output=True)
            return True
    return False

def req(method, path, body=None, headers=None, host=None):
    c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=8)
    c.putrequest(method, path, skip_host=True)
    for k, v in {"Host": host or f"127.0.0.1:{PORT}", **(headers or {})}.items(): c.putheader(k, v)
    data = json.dumps(body).encode() if body is not None else b""
    if method == "POST": c.putheader("Content-Length", str(len(data)))
    c.endheaders(data if method == "POST" else None)
    r = c.getresponse(); t = r.read().decode(); c.close(); return r.status, t

def post_download(msg, origin="chrome-extension://abcdef"):
    return req("POST", "/download", msg, {"Origin": origin})

def wait_file(name, secs=20):
    t = time.time()
    while time.time() - t < secs:
        p = os.path.join(DL, name)
        if os.path.exists(p) and os.path.getsize(p) == SIZE: return p
        time.sleep(0.5)

def exact(p):
    d = open(p, "rb").read(); return len(d) == SIZE and all(d[i] == byte(i) for i in range(0, SIZE, 997))

def hdr_log(): return [json.loads(l) for l in open(HDR_LOG) if l.strip()]
