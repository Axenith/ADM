#!/usr/bin/env python3
"""Validates the browser extension and packs it into dist/adm-extension-<version>.zip (the file to upload to the
Chrome Web Store, or to load unpacked after unzipping). Only runtime files go in: no tests, no store assets.

    python3 packaging/build-extension.py           # for GitHub releases (load unpacked): keeps the pinned `key`
    python3 packaging/build-extension.py --store   # for the Chrome Web Store: the same package without `key`
"""
import json, os, struct, sys, zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXT = os.path.join(ROOT, "extension")
manifest = json.load(open(os.path.join(EXT, "manifest.json")))
problems = []

def need(cond, msg):
    if not cond: problems.append(msg)

# Limits the Chrome Web Store enforces
need(len(manifest["name"]) <= 75, "name is longer than 75 characters")
need(len(manifest["description"]) <= 132, f"description is {len(manifest['description'])} characters (limit 132)")
need(manifest.get("manifest_version") == 3, "not a Manifest V3 extension")

# Every file the manifest points at must exist, and icons must be the size they say
refs = {manifest["background"]["service_worker"], manifest["action"]["default_popup"], manifest["options_ui"]["page"], *manifest["icons"].values(),
        *manifest["action"]["default_icon"].values()}
for r in sorted(refs):
    need(os.path.exists(os.path.join(EXT, r)), f"missing file: {r}")
for size, rel in manifest["icons"].items():
    p = os.path.join(EXT, rel)
    if os.path.exists(p):
        with open(p, "rb") as f: head = f.read(24)
        w, h = struct.unpack(">II", head[16:24])
        need((w, h) == (int(size), int(size)), f"{rel} is {w}x{h}, not {size}x{size}")

# Files that go in the package
files = []
for d, dirs, names in os.walk(EXT):
    dirs[:] = sorted(x for x in dirs if x not in ("test", "store"))
    for n in sorted(names):
        rel = os.path.relpath(os.path.join(d, n), EXT)
        if rel.endswith((".md",)) or rel.startswith("."): continue
        files.append(rel)
# Every page and script the package references must be in it
for rel in files:
    if rel.endswith(".html"):
        for part in open(os.path.join(EXT, rel)).read().split('src="')[1:] + open(os.path.join(EXT, rel)).read().split('href="')[1:]:
            ref = part.split('"')[0]
            if ref and not ref.startswith(("http", "#", "chrome")) and "?" not in ref:
                need(os.path.exists(os.path.join(EXT, ref)), f"{rel} refers to a missing file: {ref}")
if problems:
    print("Cannot package the extension:\n  - " + "\n  - ".join(problems)); sys.exit(1)

version = manifest["version"]
store = "--store" in sys.argv
# The Web Store gives an extension its own ID, so the development `key` that pins the unpacked one stays out of its package.
manifest_bytes = open(os.path.join(EXT, "manifest.json"), "rb").read()
if store:
    manifest_bytes = (json.dumps({k: v for k, v in manifest.items() if k != "key"}, indent=2) + "\n").encode()
out = os.path.join(ROOT, "dist", f"adm-extension-{version}{'-store' if store else ''}.zip")
os.makedirs(os.path.dirname(out), exist_ok=True)
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for rel in files:
        info = zipfile.ZipInfo(rel, date_time=(2026, 1, 1, 0, 0, 0))  # fixed time: the same files give the same zip
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = 0o644 << 16
        z.writestr(info, manifest_bytes if rel == "manifest.json" else open(os.path.join(EXT, rel), "rb").read())
print(f"{manifest['name']} {version}: {len(files)} files, {os.path.getsize(out) / 1024:.0f} KiB -> {os.path.relpath(out, ROOT)}")
print("  permissions:          ", ", ".join(manifest["permissions"]))
print("  optional permissions: ", ", ".join(manifest.get("optional_permissions", [])), "+ hosts", manifest.get("optional_host_permissions"))
print("  hosts at install:     ", manifest.get("host_permissions"))
