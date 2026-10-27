"""Clicks the real UI (ui/index.html + ui/app.js) in headless Brave/Chromium with a stand-in for Tauri's
`invoke`, and checks which backend commands each button calls. Needs `brave` (or set BROWSER).
    python3 tests/ui/run.py
"""
import json, os, re, subprocess, sys, tempfile
here = os.path.dirname(os.path.abspath(__file__))
ui = os.path.join(os.path.dirname(os.path.dirname(here)), "ui")
html = open(os.path.join(ui, "index.html")).read()
stub = open(os.path.join(here, "stub.html.part")).read()
test = open(os.path.join(here, "clicks.html.part")).read()
page = html.replace('<script src="app.js"></script>', stub + '<script src="app.js"></script>' + test)
page = page.replace("<head>", f'<head><base href="file://{ui}/">', 1)
# A fresh browser profile hangs on its first launch (first-run work), so keep one between runs and retry once.
profile = os.path.join(os.path.expanduser("~/.cache"), "adm-ui-test-profile")
with tempfile.TemporaryDirectory() as tmp:
    path = os.path.join(tmp, "harness.html")
    open(path, "w").write(page)
    cmd = [os.environ.get("BROWSER", "brave"), "--headless=new", "--disable-gpu", "--no-first-run", "--disable-background-networking",
           "--disable-component-update", "--disable-sync", "--no-default-browser-check", f"--user-data-dir={profile}",
           "--virtual-time-budget=6000", "--dump-dom", f"file://{path}"]
    out = ""
    for attempt in range(2):
        try:
            out = subprocess.run(cmd, capture_output=True, text=True, timeout=45).stdout
            break
        except subprocess.TimeoutExpired:
            print("browser timed out" + (" (first launch creates the profile), retrying" if attempt == 0 else ""), file=sys.stderr)
m = re.search(r'data-result="([^"]*)"', out)
if not m:
    sys.exit("no result: the browser produced no page")
res = json.loads(m.group(1).replace("&quot;", '"').replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">"))
for k, v in res.items():
    print(("PASS  " if v is True else "FAIL  ") + k + ("" if v is True else f"  -> {v}"))
ok = all(v is True for v in res.values())
print("ALL PASS" if ok else "SOME FAILED")
sys.exit(0 if ok else 1)
