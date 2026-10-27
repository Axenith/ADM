// The extension against a mocked browser (chrome.*) and a mock ADM server. Imported by basic.test.mjs and
// enhanced.test.mjs, which run it in the two modes the extension has:
//   basic     the optional `webRequest` permission is NOT granted: the extension sees only what the downloads API says
//   enhanced  it IS granted: the extension also sees the requests the browser makes (video list, exact replay)
// Tests that only make sense in one mode say so; the rest run in both.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";

const ENHANCED = process.env.ADM_TEST_MODE === "enhanced";
const basicOnly = ENHANCED ? test.skip : test;
const enhancedOnly = ENHANCED ? test : test.skip;

// ---- mock ADM ---------------------------------------------------------------------------------------
const seen = [];
let mode = "adm"; // adm | other | refuse
let blocked = []; // the sites ADM says not to capture
let rules = {}; // ADM's capture rules ({ fileExts, captureOverMB }); none = an ADM that takes everything
const server = http.createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    seen.push({ method: req.method, path: req.url, headers: req.headers, body });
    if (req.url === "/sync") {
      res.setHeader("Content-Type", "application/json");
      return res.end(JSON.stringify(mode === "other" ? { instanceId: "xdm-like", enabled: true } : { app: "adm", enabled: true, blockedHosts: blocked, ...rules }));
    }
    if (req.url === "/download") {
      res.statusCode = mode === "refuse" ? 500 : 200;
      return res.end("{}");
    }
    res.statusCode = 404;
    res.end();
  });
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const URL_ = `http://127.0.0.1:${server.address().port}`;

// ---- mock browser -----------------------------------------------------------------------------------
const store = {};
const calls = [];
const listeners = {};
let badge = "";
let icon = "", title = "";
const perm = { granted: true, asked: [] }; // the optional cookie permission, and what was asked about
const cookiesByUrl = { "https://files.example/a.zip": [{ name: "session", value: "abc" }, { name: "pref", value: "1" }] };
const session = {};
const tabBadge = {};
globalThis.chrome = {
  storage: {
    local: { get: async (d) => ({ ...d, ...store }), set: async (o) => void Object.assign(store, o) },
    session: { get: async (k) => (k in session ? { [k]: session[k] } : {}), set: async (o) => void Object.assign(session, o), remove: async (k) => void delete session[k] },
    onChanged: { addListener: (fn) => (listeners.storageChanged = fn) },
  },
  ...(ENHANCED ? { webRequest: {
    onHeadersReceived: { addListener: (fn, filter, extra) => { listeners.headers = fn; listeners.headersArgs = [filter, extra]; } },
    onSendHeaders: { addListener: (fn, filter, extra) => { listeners.sendHeaders = fn; listeners.sendHeadersArgs = [filter, extra]; } },
  } } : {}),
  cookies: { getAll: async ({ url }) => cookiesByUrl[url] ?? [] },
  downloads: {
    onDeterminingFilename: { addListener: (fn) => (listeners.filename = fn) },
    cancel: async (id) => void calls.push(["cancel", id]),
    erase: async (q) => void calls.push(["erase", q.id]),
  },
  permissions: { contains: async (q) => { perm.asked.push(q); return perm.granted; }, onAdded: { addListener: (fn) => (listeners.permAdded = fn) } },
  tabs: { create: (o) => void calls.push(["tab", o.url]), onUpdated: { addListener: (fn) => (listeners.tabUpdated = fn) }, onRemoved: { addListener: (fn) => (listeners.tabRemoved = fn) } },
  runtime: { getURL: (p) => "chrome-extension://test/" + p, onInstalled: { addListener: (fn) => (listeners.installed = fn) }, onStartup: { addListener() {} } },
  contextMenus: { create: (o) => calls.push(["menu", o.id]), onClicked: { addListener: (fn) => (listeners.menu = fn) } },
  action: { setBadgeText: (o) => { if (o.tabId !== undefined) tabBadge[o.tabId] = o.text; else badge = o.text; }, setBadgeBackgroundColor() {}, setTitle: (o) => (title = o.title), setIcon: (o) => (icon = o.path[16]) },
  alarms: { create() {}, onAlarm: { addListener: (fn) => (listeners.alarm = fn) } },
  commands: { onCommand: { addListener: (fn) => (listeners.command = fn) } },
};

const adm = await import("../adm.js");
adm.setAdmUrl(URL_);
await import("../background.js");

const reset = () => { rules = {}; for (const k of Object.keys(session)) delete session[k]; for (const k of Object.keys(tabBadge)) delete tabBadge[k]; blocked = []; seen.length = 0; calls.length = 0; perm.granted = true; perm.asked.length = 0; badge = ""; mode = "adm"; for (const k of Object.keys(store)) delete store[k]; };
const item = (extra = {}) => ({ id: 7, url: "https://files.example/a.zip", referrer: "https://files.example/page", filename: "/home/me/Downloads/a.zip", fileSize: 5_000_000, mime: "application/zip", ...extra });
let requestCounter = 0;
/**
 * Fires the browser's "choose a file name" event and waits until the extension has answered it. In enhanced mode the
 * request behind the download is announced first, as a browser does (`it.request`: false = never seen, or overrides).
 */
function download(it) {
  if (ENHANCED && it.request !== false) {
    listeners.sendHeaders({
      requestId: "r" + ++requestCounter, url: it.url, method: "GET", tabId: 1,
      requestHeaders: [{ name: "Cookie", value: "session=abc; pref=1" }, { name: "Referer", value: it.referrer }, { name: "User-Agent", value: "UA/1" }],
      ...(it.request || {}),
    });
  }
  return new Promise((resolve) => {
    const ret = listeners.filename(it, () => resolve(true));
    assert.equal(ret, true, "the listener must return true because it answers asynchronously");
  });
}

test("isAlive: only ADM counts", async () => {
  reset();
  assert.equal(await adm.isAlive(), true);
  mode = "other";
  assert.equal(await adm.isAlive(), false, "a different program answering on the port is not ADM");
  await new Promise((r) => server.close(r));
  assert.equal(await adm.isAlive(), false, "nothing listening");
  await new Promise((r) => server.listen(Number(new globalThis.URL(URL_).port), "127.0.0.1", r));
});

test("a browser download is handed to ADM and the browser's copy is removed", async () => {
  reset();
  await download(item());
  const post = seen.find((r) => r.path === "/download");
  assert.ok(post, "ADM received the download");
  const b = JSON.parse(post.body);
  assert.equal(b.url, "https://files.example/a.zip");
  assert.equal(b.cookie, "session=abc; pref=1");
  assert.equal(b.filename, "a.zip", "directories are stripped");
  assert.equal(b.fileSize, 5_000_000);
  assert.equal(b.referer, "https://files.example/page");
  assert.match(b.userAgent, /\S/);
  assert.equal(post.headers["content-type"], "text/plain", "a simple request: no CORS preflight");
  assert.deepEqual(calls.filter((c) => c[0] === "cancel" || c[0] === "erase"), [["cancel", 7], ["erase", 7]]);
});

test("ADM not running: the browser keeps the download", async () => {
  reset();
  mode = "other"; // nothing that looks like ADM
  await download(item());
  assert.equal(seen.some((r) => r.path === "/download"), false);
  assert.deepEqual(calls, [], "nothing cancelled");
});

test("ADM refuses the download: the browser keeps it", async () => {
  reset();
  mode = "refuse";
  await download(item());
  assert.ok(seen.some((r) => r.path === "/download"), "it was offered");
  assert.deepEqual(calls, [], "but not cancelled, so nothing is lost");
});

test("capture switched off, or an extension's own download: not touched", async () => {
  reset();
  store.enabled = false;
  await download(item());
  assert.equal(seen.length, 0);
  reset();
  await download(item({ byExtensionId: "someone-else" }));
  assert.equal(seen.length, 0);
});

test("wantsCapture rules", () => {
  const on = { enabled: true, minSizeMB: 0 };
  assert.equal(adm.wantsCapture({ url: "https://x/a.zip" }, on), true);
  for (const url of ["blob:https://x/1", "data:text/plain,hi", "file:///a", "ftp://x/a"]) assert.equal(adm.wantsCapture({ url }, on), false, url);
  assert.equal(adm.wantsCapture({ url: "https://x/a", fileSize: 2_000_000 }, { enabled: true, minSizeMB: 5 }), false, "below the minimum size");
  assert.equal(adm.wantsCapture({ url: "https://x/a", fileSize: 9_000_000 }, { enabled: true, minSizeMB: 5 }), true);
  assert.equal(adm.wantsCapture({ url: "https://x/a", fileSize: -1 }, { enabled: true, minSizeMB: 5 }), true, "unknown size is captured");
  assert.equal(adm.baseName("C:\\Users\\me\\a b.zip"), "a b.zip");
});

test("context menu: Download with ADM", async () => {
  reset();
  await listeners.menu({ menuItemId: "adm-link", linkUrl: "https://files.example/big.iso", pageUrl: "https://files.example/" }, {});
  const post = seen.find((r) => r.path === "/download");
  assert.equal(JSON.parse(post.body).url, "https://files.example/big.iso");
  assert.equal(JSON.parse(post.body).filename, "big.iso");
  reset(); mode = "other"; badge = "";
  await listeners.menu({ menuItemId: "adm-link", linkUrl: "https://files.example/big.iso" }, {});
  assert.equal(badge, "!", "the user is told ADM isn't running");
});

basicOnly("cookies are sent only when the user has allowed them", async () => {
  reset();
  perm.granted = false;
  await download(item());
  let b = JSON.parse(seen.find((r) => r.path === "/download").body);
  assert.equal(b.cookie, "", "no cookie permission: no cookies, but the download still goes to ADM");
  assert.ok(calls.some((c) => c[0] === "cancel"), "and is still captured");
  reset();
  await download(item());
  b = JSON.parse(seen.find((r) => r.path === "/download").body);
  assert.equal(b.cookie, "session=abc; pref=1");
  assert.deepEqual(perm.asked[0], { permissions: ["cookies"], origins: ["https://files.example/*"] }, "asks about exactly this site");
});

test("private windows are never captured", async () => {
  reset();
  await download(item({ incognito: true }));
  assert.equal(seen.length, 0, "ADM is not even contacted");
  assert.deepEqual(calls, []);
  await listeners.menu({ menuItemId: "adm-link", linkUrl: "https://files.example/a.zip" }, { incognito: true });
  assert.equal(seen.some((r) => r.path === "/download"), false, "the right-click menu respects it too");
  assert.equal(badge, "!");
});

test("a captured download is confirmed on the toolbar icon", async () => {
  reset();
  await download(item());
  assert.equal(badge, "✓");
});

test("the toolbar icon is greyed out while ADM is not running, and there is no alarm badge at rest", async () => {
  reset();
  await listeners.alarm({ name: "adm-status" });
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(icon, "icons/16.png");
  assert.equal(title, "ADM: connected");
  mode = "other";
  await listeners.alarm({ name: "adm-status" });
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(icon, "icons/off-16.png");
  assert.equal(title, "ADM: not running");
  assert.equal(badge, "", "nothing alarming for someone who has not installed ADM");
});

test("first install opens the welcome page; an update does not", async () => {
  reset();
  listeners.installed({ reason: "install" });
  assert.deepEqual(calls.filter((c) => c[0] === "tab"), [["tab", "chrome-extension://test/welcome.html"]]);
  calls.length = 0;
  listeners.installed({ reason: "update" });
  assert.deepEqual(calls.filter((c) => c[0] === "tab"), []);
});

test("sites on ADM's don't-capture list are left to the browser", async () => {
  reset();
  blocked = ["files.example"];
  await download(item({ referrer: "https://files.example/page", url: "https://cdn.other.net/a.zip" }));
  assert.equal(seen.some((r) => r.path === "/download"), false, "blocked by the page it came from");
  assert.deepEqual(calls, [], "and not cancelled");
  reset();
  blocked = ["example.org", "files.example"];
  await download(item({ referrer: "https://www.files.example/p", url: "https://files.example/a.zip" }));
  assert.equal(seen.some((r) => r.path === "/download"), false, "subdomains and the download's own site count too");
  reset();
  blocked = ["example.org"];
  await download(item());
  assert.ok(seen.some((r) => r.path === "/download"), "an unrelated entry blocks nothing");
  assert.equal(adm.siteBlocked(["x.com"], "https://notx.com/a", "https://y.com/"), false, "notx.com is not x.com");
  assert.equal(adm.siteBlocked(undefined, "https://a.com"), false);
});

// ---- Browser monitoring ---------------------------------------------------------------------------------------
const video = (url, tabId = 5, over = {}) => ({ url, tabId, statusCode: 200, responseHeaders: [{ name: "Content-Type", value: "video/mp4" }, { name: "Content-Length", value: "5000000" }], ...over });
const settle = () => new Promise((r) => setTimeout(r, 40));
const listFor = (tabId) => session[`media:${tabId}`] || [];

enhancedOnly("monitoring only watches response headers and never changes a request", () => {
  assert.equal(typeof listeners.headers, "function", "it listens for responses as soon as the permission exists");
  assert.deepEqual(listeners.headersArgs, [{ urls: ["<all_urls>"] }, ["responseHeaders"]], "no `blocking`, nothing but headers");
});

enhancedOnly("a video a page loads is listed for that tab and counted on the icon", async () => {
  reset();
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  assert.deepEqual(listFor(5).map((e) => e.url), ["https://c.example/a.mp4"]);
  assert.equal(tabBadge[5], "1");
  await listeners.headers(video("https://c.example/a.mp4")); await listeners.headers(video("https://c.example/b.mp4")); await settle();
  assert.equal(listFor(5).length, 2, "the same file again is not a new row");
  assert.equal(tabBadge[5], "2");
  assert.deepEqual(listFor(6), [], "another tab is unaffected");
});

enhancedOnly("monitoring ignores what it should", async () => {
  reset();
  await listeners.headers(video("https://c.example/a.mp4", -1)); // not from a tab
  await listeners.headers({ url: "https://c.example/p.html", tabId: 5, statusCode: 200, responseHeaders: [{ name: "content-type", value: "text/html" }] });
  await listeners.headers(video("https://c.example/tiny.mp4", 5, { responseHeaders: [{ name: "content-type", value: "video/mp4" }, { name: "content-length", value: "1000" }] }));
  await settle();
  assert.deepEqual(session, {}, "nothing was stored");
});

enhancedOnly("switching monitoring off stops listing, and on starts again", async () => {
  reset();
  store.monitoring = false; listeners.storageChanged({ monitoring: { newValue: false } }, "local");
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  assert.deepEqual(listFor(5), []);
  store.monitoring = true; listeners.storageChanged({ monitoring: { newValue: true } }, "local");
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  assert.equal(listFor(5).length, 1);
});

enhancedOnly("a new page in a tab starts a fresh list; a closed tab's list goes away", async () => {
  reset();
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  listeners.tabUpdated(5, { status: "complete" }); await settle();
  assert.equal(listFor(5).length, 1, "finishing loading changes nothing");
  listeners.tabUpdated(5, { status: "loading" }); await settle();
  assert.equal(listFor(5).length, 1, "loading without a new address changes nothing");
  listeners.tabUpdated(5, { status: "loading", url: "https://c.example/next" }); await settle();
  assert.deepEqual(listFor(5), [], "navigating away clears it");
  assert.equal(tabBadge[5], "");
  await listeners.headers(video("https://c.example/z.mp4", 8)); await settle();
  listeners.tabRemoved(8); await settle();
  assert.deepEqual(listFor(8), []);
});

enhancedOnly("monitoring starts the moment the permission is granted", () => {
  // `startWatching` is safe to call again: the listener is added once.
  const before = listeners.headers;
  listeners.permAdded();
  assert.equal(listeners.headers, before);
});

// ---- Capture rules (ADM decides which downloads it takes over) --------------------------------------------------
test("only the file types ADM lists, or big files, are taken over", async () => {
  reset();
  rules = { fileExts: ["ZIP", "ISO"], captureOverMB: 20 };
  await download(item({ url: "https://files.example/a.zip", filename: "/d/a.zip" }));
  assert.ok(seen.some((r) => r.path === "/download"), "a listed type");
  reset(); rules = { fileExts: ["ZIP"], captureOverMB: 20 };
  await download(item({ id: 8, url: "https://files.example/notes.txt", filename: "/d/notes.txt", fileSize: 4_000 }));
  assert.equal(seen.some((r) => r.path === "/download"), false, "an unlisted small file stays in the browser");
  assert.deepEqual(calls, [], "and is not cancelled");
  reset(); rules = { fileExts: ["ZIP"], captureOverMB: 20 };
  await download(item({ id: 9, url: "https://files.example/video.weird", filename: "/d/video.weird", fileSize: 30 * 1048576 }));
  assert.ok(seen.some((r) => r.path === "/download"), "an unlisted type is taken over when it is big enough");
  reset(); rules = { fileExts: ["ZIP"], captureOverMB: 0 };
  await download(item({ id: 10, url: "https://files.example/video.weird", filename: "/d/video.weird", fileSize: 30 * 1048576 }));
  assert.equal(seen.some((r) => r.path === "/download"), false, "0 turns the size rule off");
  reset(); rules = { fileExts: ["ZIP"], captureOverMB: 20 };
  await download(item({ id: 11, url: "https://files.example/x", filename: "", fileSize: -1 }));
  assert.equal(seen.some((r) => r.path === "/download"), false, "unknown type and unknown size: left alone");
});

test("an ADM that publishes no rules takes everything, as before", async () => {
  reset();
  await download(item({ url: "https://files.example/odd.thing", filename: "/d/odd.thing", fileSize: 10 }));
  assert.ok(seen.some((r) => r.path === "/download"));
});

// ---- Exact replay of what the browser sent (enhanced mode) ------------------------------------------------------
enhancedOnly("the download is fetched again with exactly what the browser sent", async () => {
  reset();
  await download(item({ request: { requestHeaders: [
    { name: "Cookie", value: "sid=exact; HttpOnly=1" }, { name: "Authorization", value: "Bearer tok" }, { name: "X-Custom", value: "kept" },
    { name: "Referer", value: "https://files.example/page" }, { name: "User-Agent", value: "UA/9" }, { name: "Accept-Language", value: "bn,en" },
    { name: "Sec-Fetch-Dest", value: "document" }, { name: "Upgrade-Insecure-Requests", value: "1" }, { name: "Range", value: "bytes=0-" },
    { name: "If-None-Match", value: "x" }, { name: "Accept-Encoding", value: "gzip" }, { name: "Origin", value: "https://files.example" },
  ] } }));
  const b = JSON.parse(seen.find((r) => r.path === "/download").body);
  assert.equal(b.cookie, "sid=exact; HttpOnly=1", "the cookies the browser really sent, not a lookup");
  const names = Object.keys(b.requestHeaders).sort();
  assert.deepEqual(names, ["Accept-Language", "Authorization", "Referer", "User-Agent", "X-Custom"], "what a download manager needs, nothing browser-private");
  assert.deepEqual(b.requestHeaders.Authorization, ["Bearer tok"]);
});

enhancedOnly("when the browser showed no cookie for the download, the cookie jar fills the gap", async () => {
  reset();
  // What a real browser reports for a download started by a click: almost nothing.
  await download(item({ request: { requestHeaders: [{ name: "Sec-GPC", value: "1" }, { name: "Accept-Language", value: "en" }] } }));
  const b = JSON.parse(seen.find((r) => r.path === "/download").body);
  assert.equal(b.cookie, "session=abc; pref=1");
  assert.deepEqual(b.requestHeaders, { "Accept-Language": ["en"] });
});

enhancedOnly("a download that was not a plain GET stays with the browser", async () => {
  reset();
  await download(item({ request: { method: "POST" } }));
  assert.equal(seen.some((r) => r.path === "/download"), false, "a file made by a form cannot be fetched again by address");
  assert.deepEqual(calls, [], "so the browser keeps it");
});

enhancedOnly("a download the browser was not seen fetching stays with the browser", async () => {
  reset();
  await download(item({ request: false }));
  assert.equal(seen.some((r) => r.path === "/download"), false);
  assert.deepEqual(calls, []);
});

enhancedOnly("a download that was redirected is matched to its first address", async () => {
  reset();
  listeners.sendHeaders({ requestId: "redir", url: "https://files.example/a.zip", method: "GET", tabId: 1, requestHeaders: [{ name: "Cookie", value: "hop=1" }] });
  listeners.sendHeaders({ requestId: "redir", url: "https://cdn.example/final/a.zip", method: "GET", tabId: 1, requestHeaders: [{ name: "Cookie", value: "hop=2" }] });
  await download(item({ url: "https://files.example/a.zip", finalUrl: "https://cdn.example/final/a.zip", request: false }));
  assert.equal(JSON.parse(seen.find((r) => r.path === "/download").body).cookie, "hop=2", "the last hop's request is what is replayed");
});

enhancedOnly("videos found on a page keep what the browser sent for them", async () => {
  reset();
  listeners.sendHeaders({ requestId: "v1", url: "https://c.example/clip.mp4", method: "GET", tabId: 5, requestHeaders: [{ name: "Cookie", value: "cdn=1" }, { name: "Referer", value: "https://c.example/watch" }, { name: "Sec-Fetch-Mode", value: "no-cors" }] });
  await listeners.headers(video("https://c.example/clip.mp4", 5, { requestId: "v1" })); await settle();
  const entry = listFor(5)[0];
  assert.deepEqual(entry.replay, { headers: { Referer: ["https://c.example/watch"] }, cookie: "cdn=1" });
});

enhancedOnly("the request memory stays small", () => {
  for (let i = 0; i < 500; i++) listeners.sendHeaders({ requestId: "m" + i, url: "https://x.example/" + i, method: "GET", tabId: 1, requestHeaders: [] });
  assert.equal(listeners.sendHeadersArgs[1].includes("extraHeaders"), true, "Cookie and Authorization are only visible with extraHeaders");
  assert.ok(listeners.sendHeadersArgs[0].types.every((t) => !["image", "script", "stylesheet", "font"].includes(t)), "images and scripts are not even watched");
});

test.after(() => server.close());
