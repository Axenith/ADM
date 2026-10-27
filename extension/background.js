import { admState, handOver, isAlive, settings, siteBlocked, wantsCapture, baseName } from "./adm.js";
import { addEntry, classify, mediaKey } from "./media.js";

// Browser downloads: decide once the browser knows the file name, hand the link to ADM, and cancel the
// browser's own copy. If ADM is not running, or refuses it, the browser downloads as usual: a download is
// never lost because of this extension.
chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  (async () => {
    let taken = false;
    try {
      const s = await settings();
      const adm = wantsCapture(item, s) ? await admState() : null;
      // ADM keeps the list of sites the user chose not to capture ("Don't capture from this site").
      if (adm && !siteBlocked(adm.blockedHosts, item.referrer, item.url)) {
        taken = await handOver({ url: item.url, referrer: item.referrer, filename: item.filename, fileSize: item.fileSize, mime: item.mime });
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
  const ok = (await isAlive()) && (await handOver({ url, referrer: info.pageUrl || tab?.url, filename: baseName(new URL(url).pathname) }));
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
  if (entry) record(details.tabId, entry);
}

let watching = false;
function startWatching() {
  if (watching || !chrome.webRequest?.onHeadersReceived) return;
  chrome.webRequest.onHeadersReceived.addListener(onHeaders, { urls: ["<all_urls>"] }, ["responseHeaders"]);
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
