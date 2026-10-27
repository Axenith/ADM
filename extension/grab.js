import { sendBatch } from "./adm.js";
import { classifyLink, isNoise, makeMatcher } from "./grab-lib.js";

const $ = (id) => document.getElementById(id);
const MAX_SEND = 200; // ADM takes this many at once
const MAX_SHOWN = 1000;
const CHIPS = [["files", "Files"], ["all", "All links"], ["video", "Video"], ["audio", "Audio"], ["archive", "Archives"], ["document", "Documents"], ["program", "Programs"], ["image", "Images"], ["page", "Pages"]];

const SCOPES = [["link", "Links on the page"], ["resource", "Page resources"]];
const PREFS = "adm-grab-prefs";

let page = null, links = [], scope = "link", filter = "files", query = "", hideNoise = true;
const picked = new Set();

// What was chosen last time (the tab, the type, the ad filter) comes back; it is only a convenience.
try { const p = JSON.parse(localStorage.getItem(PREFS) || "{}"); if (p.scope) scope = p.scope; if (p.filter) filter = p.filter; if (typeof p.hideNoise === "boolean") hideNoise = p.hideNoise; } catch { /* fine */ }
const savePrefs = () => { try { localStorage.setItem(PREFS, JSON.stringify({ scope, filter, hideNoise })); } catch { /* fine */ } };

const inScope = (l) => l.source === scope && (!hideNoise || !l.noise);
const inFilter = (l, f) => f === "all" || (f === "files" ? l.kind !== "page" : l.kind === f);
const visible = () => { const match = makeMatcher(query); return links.filter((l) => inScope(l) && inFilter(l, filter) && match(l)); };

function renderScopes() {
  $("scopes").replaceChildren(...SCOPES.map(([id, label]) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = id === scope ? "on" : "";
    b.innerHTML = `${label}<b>${links.filter((l) => l.source === id && (!hideNoise || !l.noise)).length}</b>`;
    b.addEventListener("click", () => { scope = id; savePrefs(); render(); });
    return b;
  }));
}

function renderChips() {
  $("chips").replaceChildren(...CHIPS.map(([id, label]) => {
    const n = links.filter((l) => inScope(l) && inFilter(l, id)).length;
    const b = document.createElement("button");
    b.type = "button";
    b.className = id === filter ? "on" : "";
    b.innerHTML = `${label}<b>${n}</b>`;
    b.addEventListener("click", () => { filter = id; savePrefs(); render(); });
    return b;
  }));
}

function render() {
  renderScopes();
  renderChips();
  const all = visible();
  const shown = all.slice(0, MAX_SHOWN);
  $("shown").textContent = all.length > MAX_SHOWN ? `Showing the first ${MAX_SHOWN} of ${all.length}` : `${all.length} shown`;
  $("empty").hidden = all.length > 0;
  $("links").hidden = all.length === 0;
  $("links").replaceChildren(...shown.map((l) => {
    const li = document.createElement("li");
    const box = Object.assign(document.createElement("input"), { type: "checkbox", checked: picked.has(l.url) });
    const text = document.createElement("div");
    const name = Object.assign(document.createElement("div"), { className: "name", textContent: l.name || l.text || new URL(l.url).hostname });
    const addr = Object.assign(document.createElement("div"), { className: "addr", textContent: l.url, title: l.url });
    text.append(name, addr);
    const tag = Object.assign(document.createElement("span"), { className: "tag", textContent: l.ext || l.kind });
    li.append(box, text, tag);
    const toggle = () => { box.checked ? picked.add(l.url) : picked.delete(l.url); count(); };
    box.addEventListener("change", toggle);
    li.addEventListener("click", (e) => { if (e.target !== box) { box.checked = !box.checked; toggle(); } });
    return li;
  }));
  count();
}

function count() {
  const n = picked.size;
  $("picked").textContent = `${n} selected`;
  $("send").disabled = n === 0;
  $("send").textContent = n > MAX_SEND ? `Download the first ${MAX_SEND}` : n ? `Download ${n} selected` : "Download selected";
}

$("q").addEventListener("input", () => { query = $("q").value; render(); });
$("noise").addEventListener("change", () => { hideNoise = $("noise").checked; savePrefs(); render(); });
$("invert").addEventListener("click", () => { for (const l of visible().slice(0, MAX_SHOWN)) picked.has(l.url) ? picked.delete(l.url) : picked.add(l.url); render(); });
$("all").addEventListener("click", () => { visible().slice(0, MAX_SHOWN).forEach((l) => picked.add(l.url)); render(); });
$("none").addEventListener("click", () => { picked.clear(); render(); });

$("send").addEventListener("click", async () => {
  const chosen = links.filter((l) => picked.has(l.url)).slice(0, MAX_SEND).map((l) => ({ url: l.url }));
  $("send").disabled = true;
  const accepted = await sendBatch(chosen, { referrer: page?.url });
  $("result").textContent = accepted ? `Sent ${accepted} link${accepted > 1 ? "s" : ""} to ADM.` : "Couldn't reach ADM. Is it running?";
  $("result").style.color = accepted ? "var(--ok)" : "var(--bad)";
  if (accepted) { picked.clear(); render(); } else count();
});

(async () => {
  page = (await chrome.storage.session.get("grab")).grab;
  if (!page) {
    $("source").textContent = "Open this from the ADM button on a page: “Grab all links for this page…”.";
    $("empty").hidden = false;
    return;
  }
  $("source").textContent = `${page.title || "Untitled page"} · ${new URL(page.url).host}`;
  links = page.links.map((l) => ({ source: "link", ...l, ...classifyLink(l.url), noise: isNoise(l.url) }));
  $("noise").checked = hideNoise;
  // Open on the tab that has something in it.
  if (!links.some((l) => l.source === scope)) scope = scope === "link" ? "resource" : "link";
  render();
})();
