import { admState, handOver, monitoringAccess, settings } from "./adm.js";
import { collectLinks } from "./grab-lib.js";
import { formatSize, mediaFileName, mediaKey } from "./media.js";

const $ = (id) => document.getElementById(id);
const VISIBLE = 5; // rows shown before "More formats"
const DOWNLOAD = '<svg viewBox="0 0 24 24"><path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14"/></svg>';
const CHECK = '<svg viewBox="0 0 24 24"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>';
const PLAY = '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="3"/><path d="m10.5 9.4 4.2 2.6-4.2 2.6z" fill="currentColor"/></svg>';
const NOTE = '<svg viewBox="0 0 24 24"><path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/></svg>';

let tab = null, items = [], alive = false, expanded = false, access = false, monitoring = true;

/** The tab this popup is about: the one in front (or `?tab=` when a page opens this popup directly). */
async function currentTab() {
  const wanted = new URLSearchParams(location.search).get("tab");
  if (wanted) return chrome.tabs.get(Number(wanted));
  return (await chrome.tabs.query({ active: true, currentWindow: true }))[0];
}

function say(text) {
  $("note").textContent = text;
  $("note").hidden = !text;
}

function render() {
  const shown = expanded ? items : items.slice(0, VISIBLE);
  $("count").textContent = items.length;
  $("count").hidden = items.length === 0;
  $("list").replaceChildren(...shown.map(row));
  $("listfoot").hidden = items.length === 0;
  $("more").hidden = items.length <= VISIBLE;
  $("more").textContent = expanded ? "Fewer formats" : "More formats";
  $("mon").checked = access && monitoring;
  const empty = $("empty");
  empty.hidden = items.length > 0;
  empty.textContent = !access
    ? "Browser monitoring finds the videos on a page so you can download them. Turn it on below."
    : !monitoring
      ? "Browser monitoring is off."
      : "No videos found on this page yet. Play a video and it will show up here.";
  $("banner").hidden = alive;
  $("banner").textContent = alive ? "" : "ADM is not running. Start ADM to download.";
}

function row(item) {
  const li = document.createElement("li");
  const ico = document.createElement("div");
  ico.className = "ico";
  ico.innerHTML = item.kind === "audio" ? NOTE : PLAY;
  const text = document.createElement("div");
  const title = document.createElement("div");
  title.className = "title";
  title.textContent = tab?.title || item.url;
  title.title = item.url;
  const sub = document.createElement("div");
  sub.className = "sub";
  const tag = document.createElement("span");
  tag.className = "tag";
  tag.textContent = item.ext;
  const size = document.createElement("span");
  size.textContent = formatSize(item.size);
  sub.append(tag, size);
  text.append(title, sub);
  const button = document.createElement("button");
  button.className = "dl";
  button.type = "button";
  button.title = "Download with ADM";
  button.setAttribute("aria-label", "Download with ADM");
  button.innerHTML = DOWNLOAD;
  button.disabled = !alive;
  button.addEventListener("click", () => download(item, button));
  li.append(ico, text, button);
  return li;
}

async function download(item, button) {
  const ok = await handOver({ url: item.url, referrer: tab?.url, filename: mediaFileName(item, tab?.title), fileSize: item.size, mime: item.mime });
  if (!ok) return say("Couldn't reach ADM. Is it running?");
  say("");
  button.classList.add("done");
  button.innerHTML = CHECK;
  setTimeout(() => { button.classList.remove("done"); button.innerHTML = DOWNLOAD; }, 1800);
}

async function loadItems() {
  if (!tab) return;
  const key = mediaKey(tab.id);
  items = (await chrome.storage.session.get(key))[key] || [];
  render();
}

$("more").addEventListener("click", () => { expanded = !expanded; render(); });
$("clear").addEventListener("click", async () => {
  await chrome.storage.session.remove(mediaKey(tab.id));
  chrome.action.setBadgeText({ tabId: tab.id, text: "" });
  items = [];
  render();
});
$("gear").addEventListener("click", () => chrome.runtime.openOptionsPage());

$("grab").addEventListener("click", async () => {
  try {
    // Reads the page's links only now, because the user asked (the permission is `activeTab`).
    const [found] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, func: collectLinks });
    await chrome.storage.session.set({ grab: { ...found.result, tabId: tab.id, at: Date.now() } });
    await chrome.tabs.create({ url: chrome.runtime.getURL("grab.html") });
    window.close();
  } catch {
    say("Can't read the links on this page (browser pages are off limits).");
  }
});

$("mon").addEventListener("change", async () => {
  if (!access) {
    // The permission prompt would close this popup, so it is asked for on a normal page.
    $("mon").checked = false;
    chrome.tabs.create({ url: chrome.runtime.getURL("welcome.html#monitoring") });
    return;
  }
  monitoring = $("mon").checked;
  await chrome.storage.local.set({ monitoring });
  render();
});

chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "session" && tab && changes[mediaKey(tab.id)]) loadItems();
  if (area === "local" && changes.monitoring) { monitoring = changes.monitoring.newValue !== false; render(); }
});

(async () => {
  tab = await currentTab();
  [access, monitoring, alive] = [await monitoringAccess(), (await settings()).monitoring, (await admState()) !== null];
  await loadItems();
  render();
})();
