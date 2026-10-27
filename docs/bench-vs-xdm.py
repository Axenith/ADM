import os, subprocess, time, sys, hashlib, glob, resource

# usage: python3 bench-vs-xdm.py URL [OUTPUT_DIR]
# Downloads URL with ADM's CLI and with XDM (headless), alternating, and prints time, CPU and memory for each.
# Needs `xdm-app` on the PATH, and `adm-dl` (build with: cargo build --release -p adm-cli).
import tempfile
URL = sys.argv[1]
B = sys.argv[2] if len(sys.argv) > 2 else tempfile.mkdtemp(prefix="adm-bench-")
U = URL
EXPECT = None  # set to the exact size in bytes to also check completeness
AX = os.environ.get("ADM_DL", "adm-dl")

def tree(root):
    kids = {}
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                ppid = int(open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()[1])
                kids.setdefault(ppid, []).append(int(p))
            except Exception:
                pass
    out, stack = [], [root]
    while stack:
        p = stack.pop(); out.append(p); stack += kids.get(p, [])
    return out

def pss_mb(pids):
    t = 0
    for p in pids:
        try:
            for line in open(f"/proc/{p}/smaps_rollup"):
                if line.startswith("Pss:"):
                    t += int(line.split()[1]); break
        except Exception:
            pass
    return t / 1024

def run(name, cmd):
    out = f"{B}/out-{name}"; os.makedirs(out, exist_ok=True)
    os.sync()
    t0 = time.time()
    p = subprocess.Popen(["nice", "-n", "10"] + cmd, stdout=open(f"{B}/{name}.log", "w"), stderr=subprocess.STDOUT)
    peak = 0.0
    while True:
        done, status, ru = os.wait4(p.pid, os.WNOHANG)
        if done: break
        peak = max(peak, pss_mb(tree(p.pid)))
        time.sleep(1)
    wall = time.time() - t0
    files = [f for f in glob.glob(f"{out}/*.mkv")]
    size = os.path.getsize(files[0]) if files else 0
    line = (f"{name:9s} wall={wall:6.1f}s avg={size/wall/1048576:5.2f}MB/s cpu={ru.ru_utime+ru.ru_stime:5.1f}s "
            f"peakPSS={peak:5.0f}MB size_ok={'-' if EXPECT is None else ('yes' if size == EXPECT else 'NO')} rc={os.waitstatus_to_exitcode(status)}")
    open(f"{B}/results.txt", "a").write(line + "\n"); print(line, flush=True)

open(f"{B}/results.txt", "w").close()
run("adm1", [AX, U, "-c", "8", "-o", f"{B}/out-adm1"])
run("xdm1",     ["xdm-app", "--headless", "-s", "8", "-o", f"{B}/out-xdm1", U])
run("adm2", [AX, U, "-c", "8", "-o", f"{B}/out-adm2"])
run("xdm2",     ["xdm-app", "--headless", "-s", "8", "-o", f"{B}/out-xdm2", U])
def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while c := f.read(1 << 20): h.update(c)
    return h.hexdigest()[:16]
a = glob.glob(f"{B}/out-adm1/*.mkv"); x = glob.glob(f"{B}/out-xdm1/*.mkv")
if a and x:
    open(f"{B}/results.txt", "a").write(f"sha256 adm={sha(a[0])} xdm={sha(x[0])}\n")
open(f"{B}/results.txt", "a").write("DONE\n")
