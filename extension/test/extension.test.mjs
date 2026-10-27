// Run with:  node --test extension/test/
// A mocked browser (chrome.*) plus a mock ADM server; checks the behaviour that matters most: a download is
// handed to ADM with the right data, and is never lost when ADM is missing or refuses it.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";

// ---- mock ADM ---------------------------------------------------------------------------------------
const seen = [];
let mode = "adm"; // adm | other | refuse
let blocked = []; // the sites ADM says not to capture
const server = http.createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    seen.push({ method: req.method, path: req.url, headers: req.headers, body });
    if (req.url === "/sync") {
      res.setHeader("Content-Type", "application/json");
      return res.end(JSON.stringify(mode === "other" ? { instanceId: "xdm-like", enabled: true } : { app: "adm", enabled: true, blockedHosts: blocked }));
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
  webRequest: { onHeadersReceived: { addListener: (fn, filter, extra) => { listeners.headers = fn; listeners.headersArgs = [filter, extra]; } } },
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
};

const adm = await import("../adm.js");
adm.setAdmUrl(URL_);
await import("../background.js");

const reset = () => { for (const k of Object.keys(session)) delete session[k]; for (const k of Object.keys(tabBadge)) delete tabBadge[k]; blocked = []; seen.length = 0; calls.length = 0; perm.granted = true; perm.asked.length = 0; badge = ""; mode = "adm"; for (const k of Object.keys(store)) delete store[k]; };
const item = (extra = {}) => ({ id: 7, url: "https://files.example/a.zip", referrer: "https://files.example/page", filename: "/home/me/Downloads/a.zip", fileSize: 5_000_000, mime: "application/zip", ...extra });
/** Fires the browser's "choose a file name" event and waits until the extension has answered it. */
function download(it) {
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

test("cookies are sent only when the user has allowed them", async () => {
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

test("monitoring only watches response headers and never changes a request", () => {
  assert.equal(typeof listeners.headers, "function", "it listens for responses as soon as the permission exists");
  assert.deepEqual(listeners.headersArgs, [{ urls: ["<all_urls>"] }, ["responseHeaders"]], "no `blocking`, nothing but headers");
});

test("a video a page loads is listed for that tab and counted on the icon", async () => {
  reset();
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  assert.deepEqual(listFor(5).map((e) => e.url), ["https://c.example/a.mp4"]);
  assert.equal(tabBadge[5], "1");
  await listeners.headers(video("https://c.example/a.mp4")); await listeners.headers(video("https://c.example/b.mp4")); await settle();
  assert.equal(listFor(5).length, 2, "the same file again is not a new row");
  assert.equal(tabBadge[5], "2");
  assert.deepEqual(listFor(6), [], "another tab is unaffected");
});

test("monitoring ignores what it should", async () => {
  reset();
  await listeners.headers(video("https://c.example/a.mp4", -1)); // not from a tab
  await listeners.headers({ url: "https://c.example/p.html", tabId: 5, statusCode: 200, responseHeaders: [{ name: "content-type", value: "text/html" }] });
  await listeners.headers(video("https://c.example/tiny.mp4", 5, { responseHeaders: [{ name: "content-type", value: "video/mp4" }, { name: "content-length", value: "1000" }] }));
  await settle();
  assert.deepEqual(session, {}, "nothing was stored");
});

test("switching monitoring off stops listing, and on starts again", async () => {
  reset();
  store.monitoring = false; listeners.storageChanged({ monitoring: { newValue: false } }, "local");
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  assert.deepEqual(listFor(5), []);
  store.monitoring = true; listeners.storageChanged({ monitoring: { newValue: true } }, "local");
  await listeners.headers(video("https://c.example/a.mp4")); await settle();
  assert.equal(listFor(5).length, 1);
});

test("a new page in a tab starts a fresh list; a closed tab's list goes away", async () => {
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

test("monitoring starts the moment the permission is granted", () => {
  // `startWatching` is safe to call again: the listener is added once.
  const before = listeners.headers;
  listeners.permAdded();
  assert.equal(listeners.headers, before);
});

test.after(() => server.close());
