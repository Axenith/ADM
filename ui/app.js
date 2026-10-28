"use strict";
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const list = $("list");
const rows = new Map(); // id -> { li, data, seq }
let seq = 0; // the order rows arrived in: "date added"

// ---------- formatting ----------
function bytes(n) {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (i === 0 ? n.toFixed(0) : n >= 100 ? n.toFixed(0) : n.toFixed(1)) + " " + u[i];
}
function eta(s) {
  if (s == null) return "";
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m left`;
  if (s >= 60) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s left`;
  return `${s}s left`;
}
const KINDS = {
  video: "mp4 mkv webm mov avi m4v wmv flv mpg mpeg ts 3gp",
  audio: "mp3 m4a aac flac wav ogg opus",
  archive: "zip rar 7z tar gz xz bz2 zst tgz iso img dmg cab",
  doc: "pdf doc docx xls xlsx ppt pptx epub odt ods odp txt",
  app: "exe msi deb rpm appimage apk xapk pkg msix jar",
};
function kindOf(name) {
  const ext = (name.split(".").pop() || "").toLowerCase();
  for (const [k, list] of Object.entries(KINDS)) if (list.split(" ").includes(ext)) return [k, ext];
  return ["other", ext];
}

// ---------- icons ----------
const ICON = {
  pause: '<path d="M8 5v14M16 5v14"/>',
  play: '<path d="M7 4.5v15l12-7.5z" fill="currentColor"/>',
  retry: '<path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7"/>',
  open: '<path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>',
  check: '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
  warn: '<path d="M12 8v5m0 3.5v.01M10.3 3.9 2.4 17.5A2 2 0 0 0 4.1 20.5h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>',
};
Object.assign(ICON, {
  video: '<rect x="3" y="5" width="18" height="14" rx="3"/><path d="m10.5 9.4 4.2 2.6-4.2 2.6z" fill="currentColor"/>',
  audio: '<path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/>',
  archive: '<rect x="3" y="4" width="18" height="5" rx="1.5"/><path d="M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9M10 13h4"/>',
  doc: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M9 13h6M9 17h6"/>',
  app: '<path d="M12 3 4 7v10l8 4 8-4V7z"/><path d="m4 7 8 4 8-4M12 11v10"/>',
  other: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>',
});
const svg = (name) => `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[name]}</svg>`;

// ---------- toasts ----------
function toast(msg, bad) {
  const t = document.createElement("div");
  t.className = "toast" + (bad ? " bad" : "");
  t.textContent = msg;
  $("toasts").append(t);
  setTimeout(() => t.remove(), bad ? 6000 : 3500);
}
const call = (cmd, args) => invoke(cmd, args).catch((e) => toast(String(e), true));

// ---------- rows ----------
function metaText(r) {
  const size = r.total != null ? `${bytes(r.downloaded)} of ${bytes(r.total)}` : bytes(r.downloaded);
  const dot = "<i>·</i>";
  switch (r.status) {
    case "downloading": {
      if (r.speed <= 0 && r.downloaded === 0) return "Connecting…";
      const parts = [size];
      if (r.speed > 0) parts.push(bytes(r.speed) + "/s");
      if (eta(r.eta)) parts.push(eta(r.eta));
      if (r.conns > 1) parts.push(`${r.conns} connections`);
      return parts.join(dot);
    }
    case "queued": return `Waiting in the queue${dot}${size}`;
    case "paused": return `Paused${dot}${size}`;
    case "completed": return `Completed${dot}${bytes(r.total ?? r.downloaded)}`;
    case "failed": return `${esc(r.error ?? "Failed")}${dot}${size}`;
  }
}
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

function makeRow(r) {
  const li = document.createElement("li");
  li.innerHTML =
    '<div class="ftype"></div><div class="main"><div class="name"></div><div class="meta"></div></div>' +
    '<div class="pct"></div><div class="actions"></div>' +
    '<div class="bar"><div class="fill"></div><div class="segs"></div></div>';
  li.addEventListener("dblclick", () => { if (rows.get(r.id)?.data.status === "completed") call("open_file", { id: r.id }); });
  return li;
}

function setActions(li, r) {
  const box = li.querySelector(".actions");
  if (box.dataset.for === r.status) return; // rebuilt only on a status change, so a click is never lost
  box.dataset.for = r.status;
  box.classList.remove("confirm");
  box.replaceChildren();
  const add = (icon, title, fn) => {
    const b = document.createElement("button");
    b.innerHTML = svg(icon); b.title = title; b.setAttribute("aria-label", title);
    b.addEventListener("click", fn);
    box.append(b);
  };
  const id = r.id;
  if (r.status === "downloading") add("pause", "Pause", () => call("pause", { id }));
  if (r.status === "queued") add("pause", "Take out of the queue", () => call("pause", { id }));
  if (r.status === "paused") add("play", "Resume", () => call("resume", { id }));
  if (r.status === "failed") add("retry", "Retry", () => call("resume", { id }));
  if (r.status === "completed") add("open", "Open file", () => call("open_file", { id }));
  add("folder", r.status === "completed" ? "Show in folder" : "Open download folder", () => call("reveal", { id }));
  if (r.status === "completed") {
    // A finished file may be worth keeping: say exactly what each choice does.
    add("trash", "Remove or delete…", () => ask(box, li, id, [
      ["Remove from list", "", () => call("remove", { id })],
      ["Move file to trash", "danger", () => trashFile(id)],
    ]));
  } else if (r.status === "failed") {
    add("trash", "Remove", () => call("remove", { id }));
  } else {
    // Stopping a running download throws its partial data away: ask first.
    add("trash", "Delete download", () => ask(box, li, id, [["Delete download", "danger", () => call("remove", { id })]]));
  }
}

/** Swaps a row's buttons for a short, labelled choice that goes back on its own after a few seconds. */
function ask(box, li, id, choices) {
  box.classList.add("confirm");
  box.replaceChildren();
  let timer;
  const back = () => {
    clearTimeout(timer);
    box.classList.remove("confirm");
    box.dataset.for = "";
    const entry = rows.get(id);
    if (entry) setActions(li, entry.data);
  };
  for (const [label, cls, fn] of choices) {
    const b = document.createElement("button");
    b.className = "txt " + cls;
    b.textContent = label;
    // Back to the normal buttons right away: if the action fails, the row is not left stuck in this state.
    b.addEventListener("click", () => { back(); fn(); });
    box.append(b);
  }
  const cancel = document.createElement("button");
  cancel.className = "txt";
  cancel.textContent = "Cancel";
  cancel.addEventListener("click", back);
  box.append(cancel);
  timer = setTimeout(back, 7000);
}

async function trashFile(id) {
  try {
    await invoke("remove", { id, deleteFile: true });
    toast("Moved to the trash");
  } catch (e) {
    toast(String(e), true);
  }
}

function setBar(li, r) {
  const segs = li.querySelector(".segs");
  const useSegs = r.status === "downloading" && r.segs && r.segs.length > 0;
  li.classList.toggle("segmode", useSegs);
  if (useSegs) {
    if (segs.children.length !== r.segs.length) {
      segs.replaceChildren(...r.segs.map(() => { const s = document.createElement("div"); s.className = "seg"; s.append(document.createElement("i")); return s; }));
    }
    r.segs.forEach(([start, len, done], i) => {
      const s = segs.children[i];
      s.style.left = start * 100 + "%";
      s.style.width = len * 100 + "%";
      s.firstChild.style.width = Math.min(1, done) * 100 + "%";
    });
  }
  const pct = r.status === "completed" ? 100 : r.total ? Math.min(100, (r.downloaded * 100) / r.total) : 0;
  li.querySelector(".fill").style.width = pct + "%";
  return pct;
}

function render(r) {
  let entry = rows.get(r.id);
  if (!entry) {
    entry = { li: makeRow(r), data: r, seq: ++seq };
    rows.set(r.id, entry);
    list.append(entry.li);
  }
  entry.data = r;
  const { li } = entry;
  li.dataset.status = r.status;
  const [kind] = kindOf(r.name);
  const badge = li.querySelector(".ftype");
  if (badge.dataset.kind !== kind) { badge.dataset.kind = kind; badge.innerHTML = svg(kind); }
  const name = li.querySelector(".name");
  name.textContent = r.name;
  name.title = r.url;
  li.querySelector(".meta").innerHTML = metaText(r);
  const pct = setBar(li, r);
  const pctEl = li.querySelector(".pct");
  if (r.status === "completed") { if (pctEl.dataset.s !== "c") { pctEl.dataset.s = "c"; pctEl.innerHTML = svg("check"); } }
  else if (r.status === "failed") { if (pctEl.dataset.s !== "f") { pctEl.dataset.s = "f"; pctEl.innerHTML = svg("warn"); pctEl.querySelector("svg").style.cssText = "width:22px;height:22px;color:var(--bad)"; } }
  else { pctEl.dataset.s = "p"; pctEl.textContent = r.status === "queued" ? "—" : r.total ? Math.floor(pct) + "%" : "—"; }
  setActions(li, r);
  summarize();
}

// ---------- the view: library, file type, search, sort ----------
const LIBS = { all: "All Downloads", incomplete: "Incomplete", completed: "Completed" };
const TYPES = { all: "All Types", doc: "Documents", archive: "Compressed", audio: "Music", video: "Video", app: "Programs", other: "Other" };
const SORTS = {
  "date-new": "Date added (newest first)", "date-old": "Date added (oldest first)", name: "Name (A to Z)",
  size: "Size (largest first)", type: "File type", status: "Status (active first)",
};
const ORDER = { downloading: 0, queued: 1, paused: 2, failed: 3, completed: 4 };
const view = { lib: "all", type: "all", q: "", sort: "date-new" };
try { Object.assign(view, JSON.parse(localStorage.getItem("adm-view") || "{}"), { q: "" }); } catch { /* a remembered view is a convenience */ }
if (!(view.lib in LIBS)) view.lib = "all";
if (!(view.type in TYPES)) view.type = "all";
if (!(view.sort in SORTS)) view.sort = "date-new";
const saveView = () => { try { localStorage.setItem("adm-view", JSON.stringify({ lib: view.lib, type: view.type, sort: view.sort })); } catch { /* fine */ } };

const inLib = (r) => view.lib === "all" || (view.lib === "completed" ? r.status === "completed" : r.status !== "completed");
const matches = (r) => {
  if (!view.q) return true;
  const hay = (r.name + " " + r.url).toLowerCase();
  return view.q.toLowerCase().split(/\s+/).filter(Boolean).every((w) => hay.includes(w));
};
const COMPARE = {
  "date-new": (a, b) => b.seq - a.seq,
  "date-old": (a, b) => a.seq - b.seq,
  name: (a, b) => a.data.name.localeCompare(b.data.name, undefined, { numeric: true, sensitivity: "base" }) || b.seq - a.seq,
  size: (a, b) => (b.data.total ?? b.data.downloaded) - (a.data.total ?? a.data.downloaded) || b.seq - a.seq,
  type: (a, b) => kindOf(a.data.name)[0].localeCompare(kindOf(b.data.name)[0]) || b.seq - a.seq,
  status: (a, b) => ORDER[a.data.status] - ORDER[b.data.status] || b.seq - a.seq,
};

let pending = false;
/** Redraws counts, which rows show, their order and the status bar, once per burst of updates. */
function summarize() {
  if (pending) return;
  pending = true;
  setTimeout(() => { pending = false; refresh(); }, 0);
}

function refresh() {
  const entries = [...rows.values()];
  const lib = { all: 0, incomplete: 0, completed: 0 };
  const type = { all: 0, doc: 0, archive: 0, audio: 0, video: 0, app: 0, other: 0 };
  let active = 0, speed = 0, waiting = 0, shown = 0;
  for (const e of entries) {
    const r = e.data;
    lib.all++;
    lib[r.status === "completed" ? "completed" : "incomplete"]++;
    if (r.status === "queued") waiting++;
    if (r.status === "downloading") { active++; speed += r.speed || 0; }
    // File types are counted inside the chosen library, so the numbers match what a click on them shows.
    if (inLib(r)) { type.all++; type[kindOf(r.name)[0]]++; }
  }
  for (const b of document.querySelectorAll("#lib button")) b.querySelector("b").textContent = lib[b.dataset.lib];
  for (const b of document.querySelectorAll("#types button")) {
    b.querySelector("b").textContent = type[b.dataset.type];
    if (b.dataset.type === "other") b.hidden = type.other === 0 && view.type !== "other";
  }
  const sorted = entries.sort(COMPARE[view.sort]);
  for (const e of sorted) {
    const show = inLib(e.data) && (view.type === "all" || kindOf(e.data.name)[0] === view.type) && matches(e.data);
    e.li.hidden = !show;
    if (show) shown++;
  }
  // Only touch the DOM order when it is wrong: this runs on every progress update.
  const want = sorted.map((e) => e.li);
  if (want.some((li, i) => list.children[i] !== li)) list.append(...want);

  $("st-left").textContent = (active ? `${active} downloading` : lib.all ? `${lib.all} download${lib.all > 1 ? "s" : ""}` : "Ready") + (waiting ? ` · ${waiting} waiting` : "");
  $("st-speed").textContent = active && speed > 0 ? `${bytes(speed)}/s` : "";
  $("empty").hidden = shown > 0;
  if (shown === 0) {
    const m = emptyMessage(lib.all);
    $("empty-title").textContent = m[0];
    $("empty-sub").textContent = m[1];
  }
}

function emptyMessage(total) {
  if (view.q) return ["No matches", `Nothing here matches “${view.q}”.`];
  if (total === 0) return ["No downloads yet", "Click New and paste a link. Big files are split across several connections, so they finish sooner."];
  if (view.type !== "all") return [`No ${TYPES[view.type].toLowerCase()} here`, `Nothing of this type in “${LIBS[view.lib]}”.`];
  return view.lib === "completed"
    ? ["Nothing completed yet", "Finished downloads show up here."]
    : ["Nothing in progress", "Downloads that are running, paused, waiting or failed show up here."];
}

/** Marks the chosen sidebar entries and redraws. */
function setView(patch) {
  Object.assign(view, patch);
  for (const b of document.querySelectorAll("#lib button")) b.classList.toggle("on", b.dataset.lib === view.lib);
  for (const b of document.querySelectorAll("#types button")) b.classList.toggle("on", b.dataset.type === view.type);
  saveView();
  refresh();
}
$("lib").addEventListener("click", (e) => { const b = e.target.closest("button"); if (b) setView({ lib: b.dataset.lib }); });
$("types").addEventListener("click", (e) => { const b = e.target.closest("button"); if (b) setView({ type: b.dataset.type }); });
$("q").addEventListener("input", () => setView({ q: $("q").value.trim() }));
$("q").addEventListener("keydown", (e) => { if (e.key === "Escape" && $("q").value) { $("q").value = ""; setView({ q: "" }); e.stopPropagation(); } });

// ---------- toolbar menus ----------
let openMenu = null;
function closeMenu() {
  if (!openMenu) return;
  openMenu.menu.hidden = true;
  openMenu.btn.setAttribute("aria-expanded", "false");
  openMenu = null;
}
/** Opens `menu` under `btn` with `items`: [label, action, { on }] or "-" for a divider. Closes itself on any choice. */
function showMenu(btn, menu, items) {
  if (openMenu?.menu === menu) return closeMenu();
  closeMenu();
  menu.replaceChildren(...items.map((it) => {
    if (it === "-") return document.createElement("hr");
    const [label, fn, opt = {}] = it;
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.textContent = label;
    b.classList.toggle("on", !!opt.on);
    b.addEventListener("click", () => { closeMenu(); fn(); });
    return b;
  }));
  menu.hidden = false;
  btn.setAttribute("aria-expanded", "true");
  openMenu = { btn, menu };
}
document.addEventListener("click", (e) => { if (openMenu && !openMenu.btn.contains(e.target) && !openMenu.menu.contains(e.target)) closeMenu(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenu(); });

$("tb-sort").addEventListener("click", () => showMenu($("tb-sort"), $("sort-menu"),
  Object.entries(SORTS).map(([id, label]) => [label, () => { view.sort = id; setView({}); }, { on: view.sort === id }])));

const ids = (pred) => [...rows.values()].filter((e) => pred(e.data)).map((e) => e.data.id);
const each = (list, cmd) => list.forEach((id) => call(cmd, { id }));
$("tb-more").addEventListener("click", () => showMenu($("tb-more"), $("more-menu"), [
  ["Pause all", () => each(ids((r) => r.status === "downloading" || r.status === "queued"), "pause")],
  ["Resume all", () => each(ids((r) => r.status === "paused"), "resume")],
  "-",
  ["Retry failed downloads", () => each(ids((r) => r.status === "failed"), "resume")],
  ["Clear failed downloads", () => each(ids((r) => r.status === "failed"), "remove")],
]));

// Clear: removes finished downloads from the list (never their files). It asks once, by turning red for a moment.
let clearTimer = null;
function disarmClear() {
  clearTimeout(clearTimer);
  $("tb-clear").classList.remove("armed");
  $("tb-clear").querySelector("span").textContent = "Clear";
}
$("tb-clear").addEventListener("click", () => {
  const done = ids((r) => r.status === "completed");
  if (!done.length) return toast("Nothing to clear: no finished downloads.");
  if (!$("tb-clear").classList.contains("armed")) {
    $("tb-clear").classList.add("armed");
    $("tb-clear").querySelector("span").textContent = `Clear ${done.length}?`;
    clearTimer = setTimeout(disarmClear, 4000);
    return;
  }
  disarmClear();
  each(done, "remove");
  toast(`Cleared ${done.length} finished download${done.length > 1 ? "s" : ""} from the list. The files are still on disk.`);
});

// ---------- adding links ----------
function openAdd() {
  $("add").hidden = false;
  $("url").focus();
  $("url").select();
}
function closeAdd() { $("add").hidden = true; }
$("tb-new").addEventListener("click", () => ($("add").hidden ? openAdd() : closeAdd()));
$("add-x").addEventListener("click", closeAdd);
$("url").addEventListener("keydown", (e) => { if (e.key === "Escape") closeAdd(); });
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && !document.querySelector("dialog[open]")) {
    if (e.key.toLowerCase() === "n") { e.preventDefault(); openAdd(); }
    if (e.key.toLowerCase() === "f") { e.preventDefault(); $("q").focus(); $("q").select(); }
  }
});
$("add").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const links = $("url").value.split(/\s+/).filter(Boolean);
  if (!links.length) return;
  if (!links.every((l) => /^https?:\/\//i.test(l))) return toast("That doesn't look like an http:// or https:// link.", true);
  // One link opens the "New download" window; a pasted list starts straight away.
  const ask = links.length > 1 ? false : undefined;
  for (const url of links.slice(0, 50)) {
    try { await invoke("add_download", { url, ask }); } catch (e) { return toast(String(e), true); }
  }
  $("url").value = "";
  closeAdd();
  // What was just added must be visible.
  if (view.lib === "completed" || view.type !== "all" || view.q) { $("q").value = ""; setView({ lib: "all", type: "all", q: "" }); }
});
// Pasting a link anywhere in the window fills the box.
document.addEventListener("paste", (e) => {
  if (document.activeElement === $("url") || document.activeElement === $("q") || $("settings").open) return;
  const t = (e.clipboardData?.getData("text") || "").trim();
  if (/^https?:\/\//i.test(t)) { openAdd(); $("url").value = t; }
});

// ---------- settings ----------
const dlg = $("settings");
let settingsNow = null;
$("open-settings").addEventListener("click", async () => {
  const s = (settingsNow = await invoke("get_settings"));
  $("s-conn").value = s.max_connections;
  $("s-simul").value = s.max_simultaneous;
  $("s-limit").value = s.speed_limit_kib;
  $("s-dir").value = s.download_dir;
  $("s-ask").checked = s.ask_before_download;
  $("s-sort").checked = s.sort_by_category;
  $("s-remember").checked = s.remember_folder;
  $("s-blocked").value = s.blocked_sites.join("\n");
  $("s-types").value = s.capture_extensions.map((e) => e.toLowerCase()).join(" ");
  $("s-over").value = s.capture_over_mb;
  dlg.showModal();
});
dlg.addEventListener("close", async () => {
  if (dlg.returnValue !== "save") return;
  await call("set_settings", {
    settings: {
      max_connections: Math.min(32, Math.max(1, parseInt($("s-conn").value, 10) || 8)),
      max_simultaneous: Math.min(20, Math.max(0, parseInt($("s-simul").value, 10) || 0)),
      speed_limit_kib: Math.max(0, parseInt($("s-limit").value, 10) || 0),
      download_dir: $("s-dir").value.trim(),
      ask_before_download: $("s-ask").checked,
      sort_by_category: $("s-sort").checked,
      remember_folder: $("s-remember").checked,
      capture_extensions: $("s-types").value.split(/[\s,;.]+/).filter(Boolean), // the app tidies the list
      capture_over_mb: Math.max(0, parseInt($("s-over").value, 10) || 0),
      last_folder: settingsNow?.last_folder ?? null, // kept as it is: only the app updates it
      blocked_sites: $("s-blocked").value.split(/\s+/).map((h) => h.trim().toLowerCase()).filter(Boolean),
    },
  });
  toast("Settings saved");
});

// ---------- the "New download" window ----------
const nd = $("newdl");
const queue = []; // links waiting their turn: several can arrive at once
let current = null; // the one on screen
let base = ""; // the folder the category subfolders hang off
let nameEdited = false, catEdited = false, probeToken = 0, freeTimer;

const joinPath = (dir, sub) => {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return sub ? dir.replace(/[\\/]+$/, "") + sep + sub : dir;
};

function showSize(size, resumable) {
  $("nd-size").textContent = size != null ? bytes(size) : "Unknown size";
  $("nd-size").title = resumable === false ? "The server does not support resuming" : "";
}

async function showFree() {
  const dir = $("nd-dir").value.trim();
  const free = dir ? await invoke("free_space", { dir }).catch(() => null) : null;
  const need = current?.size;
  const el = $("nd-free");
  el.textContent = free == null ? "" : `Free space ${bytes(free)}`;
  el.classList.toggle("bad", free != null && need != null && need > free);
  if (free != null && need != null && need > free) el.textContent += " — not enough for this file";
}

function setPath(sub) {
  $("nd-dir").value = joinPath(base, sub);
  showFree();
}

function setKind(name) {
  const [kind] = kindOf(name);
  $("nd-icon").dataset.kind = kind;
  $("nd-icon").innerHTML = svg(kind);
}

/** A group of links ("Grab all links") shares the window: no single name or category, a list instead. */
function setBatchMode(p) {
  const batch = !!p.batch?.length;
  $("nd-title").textContent = batch ? `Download ${p.batch.length} files` : "New download";
  $("nd-file-row").hidden = $("nd-cat-row").hidden = batch;
  $("nd-batch").hidden = !batch;
  document.querySelector(".nd-fields").classList.toggle("is-batch", batch);
  if (batch) {
    $("nd-batch-list").replaceChildren(...p.batch.map((i) => {
      const li = document.createElement("li");
      const name = document.createElement("span"), size = document.createElement("span");
      name.textContent = i.name; name.title = i.name; size.textContent = i.size != null ? bytes(i.size) : "";
      li.append(name, size);
      return li;
    }));
  }
  return batch;
}

function showNextPending() {
  if (nd.open || current || !queue.length) return;
  current = queue.shift();
  const p = current;
  nameEdited = catEdited = false;
  base = p.base_dir;
  const isBatch = setBatchMode(p);
  $("nd-name").value = isBatch ? "" : p.name;
  setKind(isBatch ? "files.zip" : p.name);
  $("nd-cat").value = p.category;
  $("nd-seg").value = p.segments;
  showSize(p.size);
  if (isBatch && p.size == null) $("nd-size").textContent = `${p.batch.length} files`;
  setPath(p.sort ? p.category : "");
  $("nd-block").hidden = !p.site;
  $("nd-site").textContent = p.site || "";
  $("nd-note").hidden = true;
  $("nd-more").textContent = queue.length ? `${queue.length} more waiting` : "";
  $("nd-go").disabled = false;
  nd.showModal();
  if (isBatch) { $("nd-dir").select(); return; } // nothing to look up for a group
  $("nd-name").select();

  // Ask the server about it too: the browser does not always know the size, or the real name.
  const token = ++probeToken;
  invoke("probe_pending", { id: p.id }).then((info) => {
    if (token !== probeToken || current !== p) return;
    p.size = p.size ?? info.size;
    showSize(p.size, info.resumable);
    if (!nameEdited && info.name) { $("nd-name").value = info.name; setKind(info.name); }
    if (!catEdited && info.category !== $("nd-cat").value) { $("nd-cat").value = info.category; if (p.sort) setPath(info.category); }
    showFree();
  }).catch(() => {
    if (token !== probeToken || current !== p) return;
    $("nd-note").textContent = "Couldn't check this link yet. You can still download it.";
    $("nd-note").hidden = false;
    if (p.size == null) showSize(null);
  });
}

function pushPending(p) {
  if (current?.id === p.id || queue.some((q) => q.id === p.id)) return;
  queue.push(p);
  if (nd.open) $("nd-more").textContent = `${queue.length} more waiting`;
  showNextPending();
}

function finishPending() {
  probeToken++;
  current = null;
  if (nd.open) nd.close();
  showNextPending();
}

async function declinePending() {
  const p = current;
  if (!p) return;
  await call("cancel_pending", { id: p.id });
  finishPending();
}

$("nd-name").addEventListener("input", () => { nameEdited = true; setKind($("nd-name").value); });
$("nd-cat").addEventListener("change", () => { catEdited = true; setPath($("nd-cat").value); });
$("nd-dir").addEventListener("input", () => { base = $("nd-dir").value; clearTimeout(freeTimer); freeTimer = setTimeout(showFree, 300); });
$("nd-browse").addEventListener("click", async () => {
  try {
    const picked = await invoke("browse_folder", { start: $("nd-dir").value.trim() || base });
    if (picked) { base = picked; setPath(""); }
  } catch (e) { toast(String(e), true); }
});
$("nd-go").addEventListener("click", async () => {
  const p = current;
  const name = $("nd-name").value.trim();
  const dir = $("nd-dir").value.trim();
  if (!p) return;
  if (!name && !p.batch?.length) return toast("Give the file a name.", true);
  if (!dir) return toast("Choose a folder to save to.", true);
  $("nd-go").disabled = true;
  try {
    // The window shows the exact folder, so it is sent as is (the category only helped pick it). `base` is the
    // folder the user chose, without a category subfolder: that is what gets remembered for next time.
    await invoke("confirm_pending", { id: p.id, name, baseDir: dir, category: "", segments: Math.min(32, Math.max(1, parseInt($("nd-seg").value, 10) || 8)), remember: base });
    finishPending();
    // A new download starts out unfinished and of any type: make sure it can be seen.
    if (view.lib === "completed" || view.type !== "all" || view.q) { $("q").value = ""; setView({ lib: "all", type: "all", q: "" }); }
  } catch (e) {
    $("nd-go").disabled = false;
    toast(String(e), true);
  }
});
$("nd-cancel").addEventListener("click", declinePending);
$("nd-x").addEventListener("click", declinePending);
nd.addEventListener("cancel", (e) => { e.preventDefault(); declinePending(); }); // Esc
nd.addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target.tagName !== "BUTTON" && e.target.tagName !== "SELECT") { e.preventDefault(); $("nd-go").click(); } });
$("nd-block").addEventListener("click", async () => {
  const p = current;
  if (!p) return;
  const site = await call("block_site", { id: p.id });
  if (site) toast(`Downloads from ${site} will stay in the browser. Start this one again from the page.`);
  finishPending();
});

// ---------- events from the app ----------
listen("dl", (e) => render(e.payload));
listen("new-download", (e) => pushPending(e.payload));
listen("removed", (e) => { rows.get(e.payload)?.li.remove(); rows.delete(e.payload); summarize(); });
invoke("list").then((all) => { all.reverse().forEach(render); setView({}); }); // oldest first, so a later row is a newer one
invoke("list_pending").then((all) => all.forEach(pushPending)); // links that arrived while no window was open
summarize();
