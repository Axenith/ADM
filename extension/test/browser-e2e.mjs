// The extension in a REAL browser (headless Brave/Chromium), driven over the DevTools protocol.
//   node extension/test/browser-e2e.mjs
// A copy of the extension is pointed at a stand-in ADM on a spare port, so a running ADM is left alone.
// Checks: a real download is handed over and cancelled; with ADM off the browser downloads it itself;
// the popup and welcome pages load without errors.
import { spawn } from "node:child_process";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const ADM_PORT = 18598, WEB_PORT = 18095, DEBUG_PORT = 19222;
const DOWNLOADS = path.join(os.homedir(), "Downloads");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, extra = "") => { results.push(ok); console.log(`${ok ? "PASS" : "FAIL"}  ${name}${extra ? "  " + extra : ""}`); };

// ---- a copy of the extension pointed at the stand-in ADM -------------------------------------------------
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "adm-ext-e2e-"));
const extDir = path.join(tmp, "extension");
fs.cpSync(path.join(here, ".."), extDir, { recursive: true, filter: (p) => !/[\\/](test|store)([\\/]|$)/.test(p) });
const patch = (f, fn) => fs.writeFileSync(path.join(extDir, f), fn(fs.readFileSync(path.join(extDir, f), "utf8")));
patch("adm.js", (s) => s.replace("127.0.0.1:8598", `127.0.0.1:${ADM_PORT}`));
patch("background.js", (s) => `globalThis.__spy = { cancels: [], suggests: 0 };
{ const c = chrome.downloads.cancel.bind(chrome.downloads); chrome.downloads.cancel = (id) => { __spy.cancels.push(id); return c(id); };
  const e = chrome.downloads.onDeterminingFilename, add = e.addListener.bind(e);
  e.addListener = (fn) => add((item, suggest) => fn(item, (...a) => { __spy.suggests++; return suggest(...a); })); }
` + s);
patch("manifest.json", (s) => {
  const m = JSON.parse(s.replace("127.0.0.1:8598", `127.0.0.1:${ADM_PORT}`));
  // What the user grants through the opt-in buttons (a headless test cannot click Chrome's prompt):
  m.permissions.push("webRequest", "cookies");
  m.host_permissions.push("<all_urls>");
  return JSON.stringify(m);
});

// ---- stand-in ADM ----------------------------------------------------------------------------------------
let admMode = "adm";
let admBlocked = [];
const admSeen = [];
const adm = http.createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    admSeen.push({ path: req.url, body, headers: req.headers });
    res.setHeader("Content-Type", "application/json");
    res.end(req.url === "/sync" ? JSON.stringify(admMode === "adm" ? { app: "adm", blockedHosts: admBlocked } : { instanceId: "other" }) : "{}");
  });
});
// ---- a tiny website with a downloadable file --------------------------------------------------------------
const web = http.createServer((req, res) => {
  if (req.url === "/page.html") {
    res.setHeader("Set-Cookie", "sid=abc123; Path=/");
    return res.end('<a id="a" href="/files/test.bin" download>one</a><a id="b" href="/files/test2.bin" download>two</a>');
  }
  if (req.url === "/video.html") {
    return res.end(`<!doctype html><title>Scenic Test Video | Free Stock</title><body>
      <a href="/files/a.zip">Archive</a> <a href="/docs/spec.pdf">Spec</a> <a href="/about">About us</a> <a href="/files/a.zip">Archive again</a>
      <script>
        fetch("/media/clip.mp4", { headers: { Range: "bytes=0-65535" } });
        fetch("/media/tiny.mp4"); fetch("/live/index.m3u8"); fetch("/media/song.mp3");
      </script>`);
  }
  if (req.url.startsWith("/media/") || req.url.startsWith("/live/")) {
    const total = { "clip.mp4": 2097152, "tiny.mp4": 10000, "song.mp3": 1048576, "index.m3u8": 4000 }[req.url.split("/").pop()] ?? 1000;
    const type = req.url.endsWith(".mp4") ? "video/mp4" : req.url.endsWith(".mp3") ? "audio/mpeg" : "application/vnd.apple.mpegurl";
    if (req.headers.range) { // a player asks for a piece: the size of the whole file is in Content-Range
      return (res.writeHead(206, { "Content-Type": type, "Content-Length": 65536, "Content-Range": `bytes 0-65535/${total}` }), res.end(Buffer.alloc(65536, 1)));
    }
    return (res.writeHead(200, { "Content-Type": type, "Content-Length": total }), res.end(Buffer.alloc(total, 1)));
  }
  const m = /^\/files\/(.+)$/.exec(req.url);
  if (!m) { res.statusCode = 404; return res.end(); }
  res.writeHead(200, { "Content-Type": "application/octet-stream", "Content-Disposition": `attachment; filename="${m[1]}"`, "Content-Length": 1048576 });
  res.end(Buffer.alloc(1048576, 7));
});
await Promise.all([new Promise((r) => adm.listen(ADM_PORT, "127.0.0.1", r)), new Promise((r) => web.listen(WEB_PORT, "127.0.0.1", r))]);

// ---- the browser -----------------------------------------------------------------------------------------
const profile = path.join(os.homedir(), ".cache", "adm-ext-test-profile");
const brave = spawn(process.env.BROWSER || "brave", [
  "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--disable-component-update",
  "--disable-sync", `--user-data-dir=${profile}`, `--remote-debugging-port=${DEBUG_PORT}`, `--load-extension=${extDir}`,
  "--disable-features=DisableLoadExtensionCommandLineSwitch", "about:blank",
], { stdio: "ignore", detached: true });
const killBrowser = () => { try { process.kill(-brave.pid, "SIGKILL"); } catch {} };
process.on("exit", killBrowser);

async function connect() {
  for (let i = 0; i < 120; i++) {
    try {
      const v = await (await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/version`)).json();
      const ws = new WebSocket(v.webSocketDebuggerUrl);
      await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
      return ws;
    } catch { await sleep(500); }
  }
  throw new Error("the browser never opened its debugging port");
}
const ws = await connect();
let id = 0; const waiting = new Map(); const events = [];
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } else events.push(m); };
const send = (method, params = {}, sessionId) => new Promise((res, rej) => { const i = ++id; waiting.set(i, (m) => (m.error ? rej(new Error(method + ": " + m.error.message)) : res(m.result))); ws.send(JSON.stringify({ id: i, method, params, sessionId })); });
const evaluate = async (sessionId, expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true }, sessionId)).result.value;
const tab = async (url) => { const { targetId } = await send("Target.createTarget", { url }); const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true }); await send("Runtime.enable", {}, sessionId); return { targetId, sessionId }; };
async function waitFor(fn, ms = 15000) { for (let t = 0; t < ms; t += 250) { const v = await fn(); if (v) return v; await sleep(250); } return null; }

try {
  // The extension's service worker is up?
  const sw = await waitFor(async () => (await send("Target.getTargets")).targetInfos.find((t) => t.type === "service_worker" && t.url.includes("/background.js")), 30000);
  check("the extension loads in a real browser (service worker running)", !!sw, sw ? sw.url.split("/")[2] : "");
  const extId = sw?.url.split("/")[2];
  assert.ok(extId, "extension did not load: this Brave may block --load-extension");

  // A: ADM running -> the download is handed over and cancelled in the browser
  for (const f of ["test.bin", "test2.bin"]) fs.rmSync(path.join(DOWNLOADS, f), { force: true });
  const page = await tab(`http://127.0.0.1:${WEB_PORT}/page.html`);
  await sleep(1500);
  admSeen.length = 0;
  await evaluate(page.sessionId, 'document.getElementById("a").click()');
  const post = await waitFor(() => admSeen.find((r) => r.path === "/download"));
  check("A: a real browser download reaches ADM", !!post);
  if (post) {
    const b = JSON.parse(post.body);
    check("A: it carries the link", b.url === `http://127.0.0.1:${WEB_PORT}/files/test.bin`, b.url);
    check("A: it carries the name the browser chose", b.filename === "test.bin", b.filename);
    check("A: it carries the referring page", b.referer === `http://127.0.0.1:${WEB_PORT}/page.html`, b.referer);
    check("A: it carries the size", b.fileSize === 1048576, String(b.fileSize));
    check("A: with cookie access granted (as after the opt-in), the site's cookies go to ADM", b.cookie === "sid=abc123", JSON.stringify(b.cookie));
    check("A: the request comes from the extension (Origin chrome-extension://)", String(post.headers.origin || "").startsWith("chrome-extension://"), post.headers.origin);
  }
  await sleep(3000);
  check("A: the browser did not also download it", !fs.existsSync(path.join(DOWNLOADS, "test.bin")));
  const swA = (await send("Target.attachToTarget", { targetId: sw.targetId, flatten: true })).sessionId;
  const spyA = await evaluate(swA, "JSON.stringify(__spy)");
  check("A: the extension cancelled the browser's copy once and answered the browser", JSON.parse(spyA).cancels.length === 1 && JSON.parse(spyA).suggests === 1, spyA);

  // B: ADM off -> the extension must leave the download alone. (Whether a headless browser then finishes the
  // download is the browser's business: see the control run below.)
  admMode = "other"; admSeen.length = 0;
  await evaluate(swA, "__spy.cancels.length = 0; __spy.suggests = 0");
  // A fresh tab: Chrome holds back a second automatic download from the same page and asks the user.
  const page2 = await tab(`http://127.0.0.1:${WEB_PORT}/page.html`);
  await sleep(1500);
  await evaluate(page2.sessionId, 'document.getElementById("b").click()');
  const spyB = await waitFor(async () => { const v = JSON.parse(await evaluate(swA, "JSON.stringify(__spy)")); return v.suggests > 0 ? v : null; }, 20000);
  check("B: with ADM off, the extension answers the browser (lets the download proceed)", spyB?.suggests === 1, JSON.stringify(spyB));
  check("B: ...and does NOT cancel the browser's download", spyB?.cancels.length === 0);
  check("B: nothing was handed to ADM", !admSeen.some((r) => r.path === "/download"));
  const dl = await evaluate(swA, 'chrome.downloads.search({ orderBy: ["-startTime"], limit: 5 })');
  console.log("  info  browser's own download item:", JSON.stringify((dl || []).map((d) => ({ file: d.url.split("/").pop(), state: d.state, error: d.error }))));
  admMode = "adm";

  // D: a site on ADM's don't-capture list is left to the browser
  admBlocked = ["127.0.0.1"]; admSeen.length = 0;
  await evaluate(swA, "__spy.cancels.length = 0; __spy.suggests = 0");
  const page3 = await tab(`http://127.0.0.1:${WEB_PORT}/page.html`);
  await sleep(1500);
  await evaluate(page3.sessionId, 'document.getElementById("a").click()');
  const spyD = await waitFor(async () => { const v = JSON.parse(await evaluate(swA, "JSON.stringify(__spy)")); return v.suggests > 0 ? v : null; }, 20000);
  check("D: a blocked site's download is left to the browser (answered, not cancelled)", spyD?.suggests === 1 && spyD.cancels.length === 0, JSON.stringify(spyD));
  check("D: and not handed to ADM", !admSeen.some((r) => r.path === "/download"));
  admBlocked = [];

  // E: browser monitoring finds the videos a page loads
  admMode = "adm"; admSeen.length = 0;
  const swE = (await send("Target.attachToTarget", { targetId: sw.targetId, flatten: true })).sessionId;
  const vpage = await tab(`http://127.0.0.1:${WEB_PORT}/video.html`);
  await sleep(3000);
  const tabId = await evaluate(swE, 'chrome.tabs.query({ url: "http://127.0.0.1:' + WEB_PORT + '/video.html" }).then((t) => t[0]?.id)');
  const found = JSON.parse(await evaluate(swE, `chrome.storage.session.get("media:${tabId}").then((o) => JSON.stringify(o["media:${tabId}"] || []))`));
  check("E: the page's videos are listed (the MP4 and the MP3)", found.length === 2 && found.some((f) => f.ext === "MP4") && found.some((f) => f.ext === "MP3"), found.map((f) => f.ext).join(","));
  check("E: the size is the whole file's (from Content-Range), not the piece", found.find((f) => f.ext === "MP4")?.size === 2097152, String(found.find((f) => f.ext === "MP4")?.size));
  check("E: a tiny video and a streaming playlist are not listed", !found.some((f) => f.url.includes("tiny") || f.url.includes("m3u8")));
  check("E: the toolbar icon counts them for that tab", (await evaluate(swE, `chrome.action.getBadgeText({ tabId: ${tabId} })`)) === "2");

  const pop = await tab(`chrome-extension://${extId}/popup.html?tab=${tabId}`);
  await sleep(2500);
  const rows = await evaluate(pop.sessionId, 'JSON.stringify([...document.querySelectorAll("#list li")].map((li) => ({ title: li.querySelector(".title").textContent, tag: li.querySelector(".tag").textContent, size: li.querySelector(".sub span:last-child").textContent })))');
  const list = JSON.parse(rows);
  check("E: the popup lists them with the page's title, type and size", list.length === 2 && list.every((r) => r.title === "Scenic Test Video | Free Stock") && list.some((r) => r.tag === "MP4" && r.size === "2.0 MB"), rows);
  check("E: the popup shows the count and the monitoring switch on", (await evaluate(pop.sessionId, 'document.getElementById("count").textContent + "|" + document.getElementById("mon").checked')) === "2|true");

  if (process.env.SHOTS_DIR) {
    await send("Emulation.setDeviceMetricsOverride", { width: 340, height: 440, deviceScaleFactor: 2, mobile: false }, pop.sessionId);
    await sleep(400);
    fs.writeFileSync(path.join(process.env.SHOTS_DIR, "popup-list.png"), Buffer.from((await send("Page.captureScreenshot", { format: "png" }, pop.sessionId)).data, "base64"));
  }
  admSeen.length = 0;
  await evaluate(pop.sessionId, '[...document.querySelectorAll("#list li")].find((li) => li.querySelector(".tag").textContent === "MP4").querySelector(".dl").click()');
  const dlPost = await waitFor(() => admSeen.find((r) => r.path === "/download"));
  check("E: the download button sends that video to ADM", !!dlPost);
  if (dlPost) {
    const b = JSON.parse(dlPost.body);
    check("E: ...named after the page", b.filename === "Scenic Test Video Free Stock.mp4", b.filename);
    check("E: ...with the right link, size and the page it came from", b.url === `http://127.0.0.1:${WEB_PORT}/media/clip.mp4` && b.fileSize === 2097152 && b.referer === `http://127.0.0.1:${WEB_PORT}/video.html`, JSON.stringify([b.url, b.fileSize, b.referer]));
  }

  // grab all links: the popup reads the page's links and opens the Grab links page
  await evaluate(pop.sessionId, 'document.getElementById("grab").click()');
  const grabT = await waitFor(async () => (await send("Target.getTargets")).targetInfos.find((t) => t.url.includes("/grab.html")), 15000);
  check("E: 'Grab all links' opens the Grab links page", !!grabT);
  if (grabT) {
    const gs = (await send("Target.attachToTarget", { targetId: grabT.targetId, flatten: true })).sessionId;
    await send("Runtime.enable", {}, gs);
    await sleep(1500);
    const chips = await evaluate(gs, 'JSON.stringify([...document.querySelectorAll("#chips button")].map((b) => b.textContent))');
    check("E: it counts the page's links by type (2 files, 3 links, the repeat counted once)", /Files2/.test(chips) && /All links3/.test(chips), chips);
    const shownNames = await evaluate(gs, 'JSON.stringify([...document.querySelectorAll("#links li .name")].map((n) => n.textContent))');
    check("E: by default it shows the files, not the plain pages", JSON.parse(shownNames).sort().join() === "a.zip,spec.pdf", shownNames);
    const scopeTabs = await evaluate(gs, 'JSON.stringify([...document.querySelectorAll("#scopes button")].map((b) => b.textContent))');
    check("E: there are two tabs, the page's links and what the page itself loads", /Links on the page3/.test(scopeTabs) && /Page resources\d+/.test(scopeTabs), scopeTabs);
    await evaluate(gs, 'document.querySelectorAll("#scopes button")[1].click()');
    const names = async () => JSON.parse(await evaluate(gs, 'JSON.stringify([...document.querySelectorAll("#links li .name")].map((n) => n.textContent))'));
    const res = await names();
    check("E: page resources list what the page fetched (the videos and the audio)", ["clip.mp4", "song.mp3"].every((n) => res.includes(n)), JSON.stringify(res));
    const filterBy = async (q) => { await evaluate(gs, `(() => { const i = document.getElementById("q"); i.value = ${JSON.stringify(q)}; i.dispatchEvent(new Event("input")); })()`); return names(); };
    check("E: a wildcard filter (*.mp3) narrows them", (await filterBy("*.mp3")).join() === "song.mp3");
    check("E: a regular expression (/clip|song/) works", (await filterBy("/clip|song/")).sort().join() === "clip.mp4,song.mp3");
    check("E: a broken expression shows nothing instead of failing", (await filterBy("/(/")).length === 0);
    await filterBy("");
    await evaluate(gs, 'document.querySelectorAll("#scopes button")[0].click()');
    check("E: back on the links tab", (await names()).sort().join() === "a.zip,spec.pdf");
    if (process.env.SHOTS_DIR) {
      await evaluate(gs, 'document.getElementById("all").click()');
      await send("Emulation.setDeviceMetricsOverride", { width: 900, height: 520, deviceScaleFactor: 1.5, mobile: false }, gs);
      await sleep(400);
      fs.writeFileSync(path.join(process.env.SHOTS_DIR, "grab.png"), Buffer.from((await send("Page.captureScreenshot", { format: "png" }, gs)).data, "base64"));
      await evaluate(gs, 'document.getElementById("none").click()');
    }
    admSeen.length = 0;
    await evaluate(gs, 'document.getElementById("all").click()');
    check("E: 'Select all shown' selects them", (await evaluate(gs, 'document.getElementById("send").textContent')) === "Download 2 selected");
    await evaluate(gs, 'document.getElementById("send").click()');
    const batch = await waitFor(() => admSeen.find((r) => r.path === "/batch"));
    check("E: sending posts one batch to ADM", !!batch);
    if (batch) {
      const b = JSON.parse(batch.body);
      check("E: ...holding exactly the chosen links", b.items.map((i) => i.url.split("/").pop()).sort().join() === "a.zip,spec.pdf", JSON.stringify(b.items));
      check("E: ...with the page as its referer", b.referer === `http://127.0.0.1:${WEB_PORT}/video.html`, b.referer);
    }
    await sleep(500);
    check("E: it tells the user it was sent", /Sent 2 links to ADM/.test(await evaluate(gs, 'document.getElementById("result").textContent')));
  }

  // clear list, then monitoring off
  const pop2 = await tab(`chrome-extension://${extId}/popup.html?tab=${tabId}`);
  await sleep(2000);
  await evaluate(pop2.sessionId, 'document.getElementById("clear").click()');
  await sleep(500);
  check("E: 'Clear list' empties the list", (await evaluate(pop2.sessionId, 'document.querySelectorAll("#list li").length + "|" + document.getElementById("empty").hidden')) === "0|false");
  await evaluate(swE, 'chrome.storage.local.set({ monitoring: false })');
  await sleep(300);
  const vpage2 = await tab(`http://127.0.0.1:${WEB_PORT}/video.html`);
  await sleep(3000);
  const tabId2 = await evaluate(swE, 'chrome.tabs.query({ url: "http://127.0.0.1:' + WEB_PORT + '/video.html" }).then((t) => t.map((x) => x.id).filter((i) => i !== ' + tabId + ').pop())');
  const stored2 = await evaluate(swE, `chrome.storage.session.get("media:${tabId2}").then((o) => (o["media:${tabId2}"] || []).length)`);
  check("E: with monitoring switched off nothing is listed", stored2 === 0, String(stored2));
  await evaluate(swE, 'chrome.storage.local.set({ monitoring: true })');

  // C: the extension\'s own pages load and work
  for (const [name, expect] of [["welcome.html", "ADM extension is installed"], ["popup.html", "ADM"], ["options.html", "ADM settings"]]) {
    const t = await tab(`chrome-extension://${extId}/${name}`);
    await sleep(2500);
    const text = await evaluate(t.sessionId, "document.body.innerText");
    check(`C: ${name} renders`, text.includes(expect), JSON.stringify(text.slice(0, 40)));
    // Welcome and options say so; the popup only warns when ADM is NOT there.
    check(`C: ${name} sees ADM as connected`, name === "popup.html" ? !/not running/i.test(text) : /connected/i.test(text), "");
    const errs = events.filter((e) => e.method === "Runtime.exceptionThrown" && e.sessionId === t.sessionId);
    check(`C: ${name} has no script errors`, errs.length === 0, errs.map((e) => e.params.exceptionDetails.text).join("; "));
    if (process.env.SHOTS_DIR) {
      await send("Emulation.setDeviceMetricsOverride", { width: name === "popup.html" ? 320 : 900, height: name === "popup.html" ? 300 : 700, deviceScaleFactor: 2, mobile: false }, t.sessionId);
      await sleep(300);
      const { data } = await send("Page.captureScreenshot", { format: "png" }, t.sessionId);
      fs.writeFileSync(path.join(process.env.SHOTS_DIR, name.replace(".html", ".png")), Buffer.from(data, "base64"));
    }
  }
} catch (e) {
  check("no unexpected error", false, String(e.stack || e));
} finally {
  for (const f of ["test.bin", "test2.bin"]) fs.rmSync(path.join(DOWNLOADS, f), { force: true });
  try { await send("Browser.close"); } catch {}
  await sleep(500); killBrowser(); adm.close();
}

// ---- control: the same click in the same headless browser with a do-nothing extension ---------------------
// (A stub that only has the `downloads` permission, so the browser's own behaviour can be read back.)
{
  const ctrlDir = path.join(tmp, "control-ext");
  fs.mkdirSync(ctrlDir, { recursive: true });
  fs.writeFileSync(path.join(ctrlDir, "manifest.json"), JSON.stringify({ manifest_version: 3, name: "control", version: "1", permissions: ["downloads"], background: { service_worker: "b.js" } }));
  fs.writeFileSync(path.join(ctrlDir, "b.js"), "// does nothing\n");
  const control = spawn(process.env.BROWSER || "brave", ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-networking",
    "--disable-sync", `--user-data-dir=${profile}`, `--remote-debugging-port=${DEBUG_PORT + 1}`, `--load-extension=${ctrlDir}`,
    "--disable-features=DisableLoadExtensionCommandLineSwitch", "about:blank"], { stdio: "ignore", detached: true });
  const kill2 = () => { try { process.kill(-control.pid, "SIGKILL"); } catch {} };
  try {
    let v; for (let i = 0; i < 80 && !v; i++) { try { v = await (await fetch(`http://127.0.0.1:${DEBUG_PORT + 1}/json/version`)).json(); } catch { await sleep(500); } }
    const w2 = new WebSocket(v.webSocketDebuggerUrl); await new Promise((r) => (w2.onopen = r));
    let n = 0; const pend = new Map();
    w2.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pend.has(m.id)) pend.get(m.id)(m); };
    const s2 = (method, params = {}, sessionId) => new Promise((r) => { const i = ++n; pend.set(i, r); w2.send(JSON.stringify({ id: i, method, params, sessionId })); });
    let ctrlSw; for (let i = 0; i < 60 && !ctrlSw; i++) { ctrlSw = (await s2("Target.getTargets")).result.targetInfos.find((t) => t.type === "service_worker" && t.url.endsWith("/b.js")); if (!ctrlSw) await sleep(500); }
    const swS = (await s2("Target.attachToTarget", { targetId: ctrlSw.targetId, flatten: true })).result.sessionId;
    const { result: { targetId } } = await s2("Target.createTarget", { url: `http://127.0.0.1:${WEB_PORT}/page.html` });
    const pageS = (await s2("Target.attachToTarget", { targetId, flatten: true })).result.sessionId;
    await sleep(1500);
    await s2("Runtime.evaluate", { expression: 'document.getElementById("b").click()', userGesture: true }, pageS);
    await sleep(5000);
    const r = await s2("Runtime.evaluate", { expression: 'chrome.downloads.search({}).then(l => JSON.stringify(l.map(d => ({ file: d.url.split("/").pop(), state: d.state, error: d.error }))))', awaitPromise: true, returnByValue: true }, swS);
    const list = JSON.parse(r.result.result.value);
    console.log("  info  control (do-nothing extension, same headless browser):", JSON.stringify(list));
    check("control: the headless browser itself cancels the download (USER_CANCELED), so B's end state is not caused by ADM's extension", list.some((d) => d.file === "test2.bin" && d.error === "USER_CANCELED"));
    await s2("Browser.close");
  } catch (e) { check("control run", false, String(e)); } finally { await sleep(500); kill2(); }
}
web.close(); fs.rmSync(tmp, { recursive: true, force: true });
console.log(results.every(Boolean) ? "\nALL PASS" : "\nSOME FAILED");
process.exit(results.every(Boolean) ? 0 : 1);
