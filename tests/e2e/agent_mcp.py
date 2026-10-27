"""ADM for AI agents: drives `adm-dl mcp` the way an agent's MCP client would (JSON-RPC over stdio) against a local
range-capable file server, and checks the plain CLI's --json mode. Isolated like the other e2e scripts."""
import hashlib, json, os, queue, subprocess, sys, threading, time
from common import *
from common import _procs

BIN = os.environ.get("ADM_DL_BIN", f"{ROOT}/target/release/adm-dl")
start_hdr_server()
BASE = f"http://127.0.0.1:{WEB_PORT}"
WANT = hashlib.sha256(bytes(byte(i) for i in range(SIZE))).hexdigest()
ENV = dict(os.environ, XDG_DATA_HOME=f"{TMP}/xdg", ADM_DOWNLOAD_DIR=DL)

class Client:
    def __init__(self, **env):
        self.p = subprocess.Popen([BIN, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd=TMP,
                                  env=dict(ENV, **env), start_new_session=True)
        _procs.append(self.p)
        self.q, self.notes, self.n = queue.Queue(), [], 0
        threading.Thread(target=self._read, daemon=True).start()
    def _read(self):
        for line in self.p.stdout:
            m = json.loads(line)  # anything on stdout that is not JSON would break a real client: fail loudly
            (self.q.put(m) if "id" in m else self.notes.append(m))
    def rpc(self, method, params=None, timeout=90):
        self.n += 1; i = self.n
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params or {}}) + "\n"); self.p.stdin.flush()
        end = time.time() + timeout
        while time.time() < end:
            try: m = self.q.get(timeout=0.2)
            except queue.Empty: continue
            if m["id"] == i: return m
            self.q.put(m); time.sleep(0.01)
        raise TimeoutError(method)
    def tool(self, name, meta=None, **args):
        params = {"name": name, "arguments": args}
        if meta: params["_meta"] = meta
        r = self.rpc("tools/call", params)["result"]
        return r["structuredContent"], r["isError"]

c = Client()
r = c.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}})["result"]
check("initialize: answers with the client's protocol version and says who it is", r["protocolVersion"] == "2025-06-18" and r["serverInfo"]["name"] == "adm", json.dumps(r["serverInfo"]))
check("initialize: tells the agent when to use ADM", "instead of curl" in r["instructions"])
c.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"); c.p.stdin.flush()
check("an old client's protocol version is answered with one the server speaks", c.rpc("initialize", {"protocolVersion": "1999-01-01"})["result"]["protocolVersion"] in ("2025-06-18",))
names = [t["name"] for t in c.rpc("tools/list")["result"]["tools"]]
check("tools/list: four tools", sorted(names) == ["adm_cancel", "adm_download", "adm_probe", "adm_status"], str(names))
check("ping answers", c.rpc("ping")["result"] == {})
check("an unknown method is a JSON-RPC error, not silence", c.rpc("nope")["error"]["code"] == -32601)

v, err = c.tool("adm_probe", url=f"{BASE}/data.bin")
check("probe: size, resumable and name without downloading", v["size_bytes"] == SIZE and v["resumable"] and v["file_name"] == "data.bin" and not err, json.dumps(v))

t0 = time.time()
v, err = c.tool("adm_download", url=f"{BASE}/data.bin")
check("download: completes within the call", v["status"] == "completed" and not err, json.dumps(v))
path = v.get("path", "")
check("download: the file is in the download folder, byte-exact", path == f"{DL}/data.bin" and os.path.exists(path) and exact(path), path)
check("download: reports size, speed and time", v.get("size_bytes") == SIZE and v.get("avg_speed_bytes_per_sec", 0) > 0 and "seconds" in v)

v, err = c.tool("adm_download", url=f"{BASE}/data.bin")
check("a second download of the same link never overwrites", v["status"] == "completed" and v["path"] == f"{DL}/data (1).bin" and exact(f"{DL}/data.bin"), v.get("path"))

v, err = c.tool("adm_download", url=f"{BASE}/c.bin", sha256=WANT, file_name="checked.bin")
check("sha256 that matches: completed, and the hash is reported", v["status"] == "completed" and v["sha256"] == WANT and v["path"] == f"{DL}/checked.bin", json.dumps(v))
v, err = c.tool("adm_download", url=f"{BASE}/d.bin", sha256="0" * 64)
check("sha256 that does not match: an error that names both hashes and the file", err and v["status"] == "failed" and WANT in v["error"] and "do not use it" in v["error"], json.dumps(v))

v, err = c.tool("adm_download", url=f"{BASE}/gone/x.bin")
check("a 404 is an error with a plain message", err and v["status"] == "failed" and "HTTP 404" in v["error"], json.dumps(v))
v, err = c.tool("adm_download", url="ftp://example.com/x")
check("only http(s) links are accepted", err and "http" in v["error"])
v, err = c.tool("adm_download")
check("no url is an error", err)

v, err = c.tool("adm_download", urls=[f"{BASE}/b1.bin", f"{BASE}/b2.bin"], dest_dir="batch")
check("several links: one job each, into a created sub-folder", not err and [j["status"] for j in v["jobs"]] == ["completed"] * 2 and os.path.exists(f"{DL}/batch/b1.bin") and os.path.exists(f"{DL}/batch/b2.bin"), json.dumps(v)[:200])

# a long download: the call returns early with a job, and status waits for it
v, err = c.tool("adm_download", meta={"progressToken": "tok1"}, url=f"{BASE}/slow/long.bin", wait_seconds=2, connections=2)
check("slow file: the call returns after wait_seconds with a running job", v["status"] == "running" and v["job_id"] and not err, json.dumps(v))
job = v["job_id"]
time.sleep(1.2)
check("slow file: progress notifications are sent when the client asks for them", any(n["method"] == "notifications/progress" and n["params"]["progressToken"] == "tok1" for n in c.notes), str(len(c.notes)))
s, err = c.tool("adm_status", job_id=job)
check("status: shows progress while running", s["status"] == "running" and s["downloaded_bytes"] > 0 and s["total_bytes"] == SIZE and "percent" in s, json.dumps(s))
s, err = c.tool("adm_status", job_id=job, wait_seconds=90)
check("status with wait_seconds: waits for the end and returns the finished file", s["status"] == "completed" and exact(s["path"]), json.dumps(s))
check("status without a job lists the session's jobs", len(c.tool("adm_status")[0]["jobs"]) >= 6)
check("status of an unknown job is an error", c.tool("adm_status", job_id="job-999")[1])

v, err = c.tool("adm_download", url=f"{BASE}/slow/cancel.bin", wait_seconds=0, connections=2)
check("wait_seconds 0 answers at once", v["status"] in ("running", "queued"), json.dumps(v))
time.sleep(1.5)
check("cancel: accepted", not c.tool("adm_cancel", job_id=v["job_id"])[1])
s, err = c.tool("adm_status", job_id=v["job_id"], wait_seconds=10)
check("cancel: the job ends cancelled", s["status"] == "cancelled", json.dumps(s))
check("cancel: nothing is left behind", not any("cancel" in f for f in os.listdir(DL)) and not any(f.endswith(".admpart") for f in os.listdir(DL)), str(os.listdir(DL)))

# several asks for the same running link share one job
a, _ = c.tool("adm_download", url=f"{BASE}/slow/same.bin", wait_seconds=0)
b, _ = c.tool("adm_download", url=f"{BASE}/slow/same.bin", wait_seconds=0)
check("asking for a link that is already downloading returns the same job", a["job_id"] == b["job_id"])
c.tool("adm_cancel", job_id=a["job_id"])

# confinement
root = f"{TMP}/sandbox"; os.makedirs(root)
c2 = Client(ADM_MCP_ROOT=root, ADM_DOWNLOAD_DIR=root)
c2.rpc("initialize", {"protocolVersion": "2025-06-18"})
v, err = c2.tool("adm_download", url=f"{BASE}/ok.bin", dest_dir="sub")
check("ADM_MCP_ROOT: a folder inside it is fine", not err and v["status"] == "completed" and v["path"].startswith(root), json.dumps(v)[:160])
for bad in ("../escape", "/tmp/elsewhere"):
    v, err = c2.tool("adm_download", url=f"{BASE}/no.bin", dest_dir=bad)
    check(f"ADM_MCP_ROOT: {bad!r} is refused", err and "ADM_MCP_ROOT" in v["error"], json.dumps(v)[:160])
os.symlink("/tmp", f"{root}/link")
v, err = c2.tool("adm_download", url=f"{BASE}/no.bin", dest_dir="link")
check("ADM_MCP_ROOT: a symlink out of it is refused", err and "ADM_MCP_ROOT" in v["error"], json.dumps(v)[:160])
check("ADM_MCP_ROOT: nothing was written outside", not os.path.exists("/tmp/no.bin") and not os.path.exists("/tmp/elsewhere"))
c.p.stdin.close(); c2.p.stdin.close()
time.sleep(0.3)
check("the server exits when the client goes away", c.p.poll() is not None and c2.p.poll() is not None)

# the plain command line, as an agent would run it in a shell
out = f"{TMP}/cli"
r = subprocess.run([BIN, f"{BASE}/cli.bin", "-o", out, "--sha256", WANT, "--json"], capture_output=True, text=True, env=ENV, timeout=60)
j = json.loads(r.stdout.strip().splitlines()[-1])
check("CLI --json: one JSON line on stdout, exit 0", r.returncode == 0 and len(r.stdout.strip().splitlines()) == 1 and j["status"] == "completed" and j["path"] == f"{out}/cli.bin" and j["sha256"] == WANT, r.stdout[:200])
check("CLI --json: no carriage-return progress storm when not on a terminal", "\r" not in r.stderr, repr(r.stderr[-120:]))
r = subprocess.run([BIN, f"{BASE}/gone/y.bin", "-o", out, "--json"], capture_output=True, text=True, env=ENV, timeout=60)
j = json.loads(r.stdout.strip().splitlines()[-1])
check("CLI --json on a 404: exit 1 and a JSON error", r.returncode == 1 and j["status"] == "failed" and "HTTP 404" in j["error"], r.stdout[:200])
r = subprocess.run([BIN, f"{BASE}/z.bin", "-o", out, "-n", "../../evil.bin", "--sha256", "0" * 64, "--json"], capture_output=True, text=True, env=ENV, timeout=60)
check("CLI --sha256 mismatch: exit 1, and -n cannot escape the folder", r.returncode == 1 and "checksum mismatch" in r.stdout and not os.path.exists(f"{TMP}/evil.bin"), r.stdout[:200])

print("\nALL PASS" if ok else "\nSOME FAILED"); sys.exit(0 if ok else 1)
