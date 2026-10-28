import { admState, capturedByRules, handOver, isAlive, lastKnownRules, settings, siteBlocked, toReplay, wantsCapture, baseName } from "./adm.js";
import { addEntry, classify, mediaKey } from "./media.js";
import { openGrab } from "./grab-open.js";

// Browser downloads: decide once the browser knows the file name, hand the link to ADM, and cancel the
// browser's own copy. If ADM is not running, or refuses it, the browser downloads as usual: a download is
// never lost because of this extension.
chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  (async () => {
    let taken = false;
    try {
      const s = await settings();
      let adm = wantsCapture(item, s) ? await admState() : null;
      // ADM is off: judge the download by what ADM last said it takes (it is started when it is taken). A browser
      // that has never seen ADM has no rules, so nothing is started for it.
      if (!adm && s.autoStart && wantsCapture(item, s)) adm = await lastKnownRules();
      // ADM keeps the capture rules and the list of sites the user chose not to capture.
      if (adm && capturedByRules(item, adm) && !siteBlocked(adm.blockedHosts, item.referrer, item.url)) {
        let replay;
        let leave = false;
        if (watching) {
          // With the permission, what the browser really asked for is known. Only a plain GET can be fetched again:
          // a file made by a form (POST) or one the browser did not fetch itself stays with the browser.
          const seen = findObserved(item);
          if (!seen || seen.method !== "GET") leave = true;
          else replay = toReplay(seen.headers);
        }
        if (!leave) {
          taken = await handOver({ url: item.url, referrer: item.referrer, filename: item.filename, fileSize: item.fileSize, mime: item.mime, replay });
        }
      }
    } catch (e) {
      console.warn("ADM: could not hand over the download", e);
    }
    if (taken) {
      try {
        await chrome.downloads.cancel(item.id);
        await chrome.downloads.erase({ id: item.id });
      } catch (e) {
        console.warn("ADM: could not remove the browser's copy", e);
      }
      flash("✓", "#14966a");
    }
    try { suggest(); } catch { /* the download is already cancelled */ }
  })();
  return true; // `suggest` is called asynchronously
});

// Right-click a link (or a video/audio) -> "Download with ADM".
chrome.runtime.onInstalled.addListener((details) => {
  chrome.contextMenus.create({ id: "adm-link", title: "Download with ADM", contexts: ["link", "video", "audio"] });
  refreshStatus();
  // First run: a short page that explains that ADM is needed and offers the optional cookie permission.
  if (details?.reason === "install") chrome.tabs.create({ url: chrome.runtime.getURL("welcome.html") });
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId !== "adm-link") return;
  const url = info.linkUrl || info.srcUrl;
  if (!/^https?:/i.test(url || "")) return;
  if (tab?.incognito) return flash("!", "#dc3f55");
  const ok = await handOver({ url, referrer: info.pageUrl || tab?.url, filename: baseName(new URL(url).pathname) });
  if (ok) flash("✓", "#14966a");
  else flash("!", "#dc3f55");
});

// The toolbar icon is in colour while ADM is running and greyed out while it is not. No badge is shown at
// rest: someone who has not installed ADM yet should not see an alarm on their toolbar.
async function refreshStatus() {
  const alive = await isAlive();
  const prefix = alive ? "" : "off-";
  chrome.action.setIcon({ path: { 16: `icons/${prefix}16.png`, 32: `icons/${prefix}32.png`, 48: `icons/${prefix}48.png` } });
  chrome.action.setTitle({ title: alive ? "ADM: connected" : "ADM: not running" });
}

/** A short confirmation (or warning) on the toolbar icon. */
function flash(text, color) {
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setBadgeText({ text });
  setTimeout(() => chrome.action.setBadgeText({ text: "" }), 3000);
}

chrome.alarms.create("adm-status", { periodInMinutes: 1 });
chrome.alarms.onAlarm.addListener((a) => { if (a.name === "adm-status") refreshStatus(); });
chrome.runtime.onStartup.addListener(refreshStatus);
// Alt+Shift+G (changeable at chrome://extensions/shortcuts): grab the links of the page in front.
chrome.commands.onCommand.addListener((name, tab) => { if (name === "grab-links" && tab?.id != null) openGrab(tab.id).catch(() => {}); });
refreshStatus();

// ---- Browser monitoring: find the videos and audio a page loads --------------------------------------------
// Only active once the user has allowed it (the `webRequest` permission is optional), and only while the setting
// is on. It listens to response headers only: it never blocks or changes a request.
let monitoringOn = true; // until the setting has been read
const ready = settings().then((s) => { monitoringOn = s.monitoring; });
chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "local" && changes.monitoring) monitoringOn = changes.monitoring.newValue !== false;
});

let chain = Promise.resolve(); // many responses arrive at once: record them one at a time
function record(tabId, entry) {
  chain = chain.then(async () => {
    const key = mediaKey(tabId);
    const current = (await chrome.storage.session.get(key))[key] || [];
    const next = addEntry(current, entry);
    if (next === current) return;
    await chrome.storage.session.set({ [key]: next });
    chrome.action.setBadgeBackgroundColor({ tabId, color: "#0b6cff" });
    chrome.action.setBadgeText({ tabId, text: String(next.length) });
  }).catch((e) => console.warn("ADM: could not record a video", e));
}

async function onHeaders(details) {
  if (details.tabId < 0) return; // not from a tab (a service worker, the browser itself)
  await ready;
  if (!monitoringOn) return;
  const entry = classify(details);
  if (!entry) return;
  const seen = observed.get(details.requestId);
  record(details.tabId, seen ? { ...entry, replay: toReplay(seen.headers) } : entry);
}

// What the browser actually sent, kept briefly: a download's request is looked up here when it turns into a download.
const observed = new Map(); // requestId -> { url, originUrl, method, headers, tabId, ts }
const OBSERVED_TTL_MS = 60_000, OBSERVED_MAX = 200;

function onSendHeaders(d) {
  const prev = observed.get(d.requestId);
  // A redirect re-fires this under the same id: remember where the chain started (a download's own `url`).
  observed.delete(d.requestId);
  observed.set(d.requestId, { url: d.url, originUrl: prev?.originUrl ?? d.url, method: d.method, headers: d.requestHeaders || [], tabId: d.tabId, ts: Date.now() });
  for (const [id, rec] of observed) {
    if (observed.size <= OBSERVED_MAX && Date.now() - rec.ts <= OBSERVED_TTL_MS) break;
    observed.delete(id); // oldest first
  }
}

function findObserved(item) {
  for (const rec of [...observed.values()].reverse()) {
    if (rec.originUrl === item.url || rec.url === item.url || (item.finalUrl && rec.url === item.finalUrl)) return rec;
  }
  return null;
}

let watching = false;
function startWatching() {
  if (watching || !chrome.webRequest?.onHeadersReceived) return;
  chrome.webRequest.onHeadersReceived.addListener(onHeaders, { urls: ["<all_urls>"] }, ["responseHeaders"]);
  // Only the kinds of request that can become a download or a video, not every image and script on a page.
  chrome.webRequest.onSendHeaders.addListener(onSendHeaders, { urls: ["<all_urls>"], types: ["main_frame", "sub_frame", "xmlhttprequest", "media", "other", "object"] }, ["requestHeaders", "extraHeaders"]);
  watching = true;
}
startWatching(); // when the permission is already there
chrome.permissions.onAdded.addListener(startWatching); // or as soon as it is granted

// A new page in a tab starts a new list; a closed tab's list goes away.
chrome.tabs.onUpdated.addListener((tabId, info) => {
  if (info.status === "loading" && info.url) {
    chrome.storage.session.remove(mediaKey(tabId));
    chrome.action.setBadgeText({ tabId, text: "" });
  }
});
chrome.tabs.onRemoved.addListener((tabId) => chrome.storage.session.remove(mediaKey(tabId)));
