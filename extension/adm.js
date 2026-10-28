// Everything that talks to ADM, shared by the background worker, the popup and the welcome page.
//
// ADM listens on 127.0.0.1 only, and only answers requests that come from a browser extension (a web page's
// requests are refused by ADM), so nothing here is reachable from the sites you visit. Nothing is ever sent to
// any other server.

export let ADM_URL = "http://127.0.0.1:8598";
/** For tests. */
export function setAdmUrl(url) { ADM_URL = url; }

/** Where people can get ADM. Empty until ADM has a public home; the pages hide the link while it is empty. */
export const ADM_HOME = "";

export const DEFAULTS = { enabled: true, minSizeMB: 0, monitoring: true, autoStart: true };

export async function settings() {
  const stored = await chrome.storage.local.get(DEFAULTS);
  return { ...DEFAULTS, ...stored };
}

async function admFetch(path, init = {}, timeoutMs = 1500) {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  try {
    return await fetch(ADM_URL + path, { cache: "no-store", ...init, signal: ctl.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** ADM's answer to `/sync` (its settings, including the sites it should not be given), or null if ADM is not there. */
export async function admState() {
  try {
    const res = await admFetch("/sync");
    if (!res.ok) return null;
    const body = await res.json();
    if (body?.app !== "adm") return null; // not any other program that happens to answer on the port
    rememberRules(body);
    return body;
  } catch {
    return null;
  }
}

// What ADM last said about what it takes over, kept so a download can be recognised while ADM is off.
function rememberRules(body) {
  const rules = { fileExts: body.fileExts, captureOverMB: body.captureOverMB, blockedHosts: body.blockedHosts || [] };
  Promise.resolve(chrome.storage.local.set({ admRules: rules })).catch(() => {});
}

/** ADM's rules as of the last time it was running, or null if it has never been seen. */
export async function lastKnownRules() {
  const { admRules } = await chrome.storage.local.get({ admRules: null });
  return admRules;
}

/** The program that starts ADM for the extension (see src-tauri/src/native_host.rs). */
export const HOST = "dev.adm.launcher";
let starting = null;

/**
 * ADM's state, starting ADM first if it is not running (and the user has not turned that off). Clicking "download"
 * should work whether or not ADM was already open. Resolves null if ADM cannot be reached: it was never run (so the
 * browser does not know where it is), or it did not come up.
 */
export async function ensureAdm() {
  const now = await admState();
  if (now || !(await settings()).autoStart) return now;
  // Several downloads at once start ADM once.
  starting ??= (async () => {
    try {
      const r = await chrome.runtime.sendNativeMessage(HOST, { op: "start" });
      if (!r?.ok) return null;
    } catch {
      return null; // the browser has no record of ADM yet
    }
    for (let i = 0; i < 40; i++) { // up to about 12 seconds
      await new Promise((r) => setTimeout(r, 300));
      const s = await admState();
      if (s) return s;
    }
    return null;
  })().finally(() => { starting = null; });
  return starting;
}

/** True only for ADM itself. */
export async function isAlive() {
  return (await admState()) !== null;
}

const hostOf = (url) => { try { return new URL(url).hostname.toLowerCase(); } catch { return null; } };

/** Whether the page a download came from, or the download's own site, is on the user's "don't capture" list. */
export function siteBlocked(blockedHosts, ...urls) {
  const hosts = urls.map(hostOf).filter(Boolean);
  return (blockedHosts || []).some((b) => hosts.some((h) => h === b || h.endsWith("." + b)));
}

/** Whether the user has allowed reading cookies for this site (an opt-in: see `requestCookieAccess`). */
export async function cookieAccess(url) {
  try {
    return await chrome.permissions.contains({ permissions: ["cookies"], origins: [new URL(url).origin + "/*"] });
  } catch {
    return false;
  }
}

/** Whether cookie access was granted for all sites (what the popup's switch controls). */
export async function cookieAccessEverywhere() {
  try {
    return await chrome.permissions.contains({ permissions: ["cookies"], origins: ["<all_urls>"] });
  } catch {
    return false;
  }
}

/** Must be called from a click: asks Chrome to show its permission prompt. Resolves true if granted. */
export function requestCookieAccess() {
  return chrome.permissions.request({ permissions: ["cookies"], origins: ["<all_urls>"] });
}

export function dropCookieAccess() {
  return chrome.permissions.remove({ permissions: ["cookies"], origins: ["<all_urls>"] });
}

/** Whether the user has allowed watching page loads to find videos ("Browser monitoring"). An opt-in permission. */
export async function monitoringAccess() {
  try {
    return await chrome.permissions.contains({ permissions: ["webRequest"], origins: ["<all_urls>"] });
  } catch {
    return false;
  }
}

/** Must be called from a click: asks Chrome to show its permission prompt. Resolves true if granted. */
export function requestMonitoringAccess() {
  return chrome.permissions.request({ permissions: ["webRequest"], origins: ["<all_urls>"] });
}

export function dropMonitoringAccess() {
  return chrome.permissions.remove({ permissions: ["webRequest"] });
}

const cookieHeader = (cookies) => cookies.map((c) => `${c.name}=${c.value}`).join("; ");

/** The cookies the site would see, or "" if the user has not allowed access (public downloads still work). */
async function cookiesFor(url) {
  if (!(await cookieAccess(url))) return "";
  try {
    return cookieHeader(await chrome.cookies.getAll({ url }));
  } catch {
    return "";
  }
}

/** `C:\a\b.zip` or `/a/b.zip` -> `b.zip` */
export function baseName(path) {
  return (path || "").split(/[\\/]/).pop() || undefined;
}

/** Whether a browser download should go to ADM instead. */
export function wantsCapture(item, s) {
  if (!s.enabled) return false;
  if (!/^https?:/i.test(item.url || "")) return false; // blob:, data:, file:, ...
  if (item.byExtensionId) return false; // started by an extension: leave it alone
  // Private windows: their cookies and links must not be written to ADM's list on disk.
  if (item.incognito) return false;
  if (s.minSizeMB > 0 && item.fileSize > 0 && item.fileSize < s.minSizeMB * 1048576) return false;
  return true;
}

/**
 * Whether ADM's capture rules (set in ADM's Settings, sent with `/sync`) want this download: its file type is on the
 * list, or it is at least `captureOverMB` big. An ADM that does not send rules takes everything, as before.
 */
export function capturedByRules(item, adm) {
  if (!Array.isArray(adm?.fileExts)) return true;
  let name = baseName(item.filename);
  if (!name) { try { name = decodeURIComponent(new URL(item.url).pathname.split("/").pop() || ""); } catch { name = ""; } }
  const ext = /\.([A-Za-z0-9]+)$/.exec(name)?.[1]?.toUpperCase();
  if (ext && adm.fileExts.includes(ext)) return true;
  const over = Number(adm.captureOverMB) || 0;
  return over > 0 && item.fileSize > 0 && item.fileSize >= over * 1048576;
}

// Headers a browser adds for itself. They mean nothing to a download manager (or would change how a server answers),
// and ADM sets its own range and encoding. `Cookie` is handled apart.
const NOT_REPLAYED = /^(cookie|host|connection|content-length|content-type|origin|range|if-.+|sec-.+|upgrade-insecure-requests|accept-encoding|priority|te|dnt|pragma|cache-control)$/i;

/** What the browser really sent for a request: `{ headers: { Name: [value] }, cookie }`, ready for ADM. */
export function toReplay(requestHeaders) {
  const headers = {};
  let cookie;
  for (const { name, value } of requestHeaders || []) {
    if (value == null) continue;
    if (name.toLowerCase() === "cookie") cookie = value;
    else if (!NOT_REPLAYED.test(name)) (headers[name] ||= []).push(value);
  }
  return { headers, cookie };
}

/**
 * Gives ADM the link plus what the browser knows: the cookies the site would see (if allowed), the page it came
 * from, the name the browser chose. Resolves true only if ADM accepted it.
 */
export async function handOver({ url, referrer, filename, fileSize, mime, replay }) {
  if (!(await ensureAdm())) return false;
  const body = {
    url,
    // The cookies the browser really sent, when it was watched; else the ones it would send (if allowed).
    // (The browser does not always show a download's cookies to extensions (a download started by a click is reported
    // with almost no headers), so what it did show wins, and the cookie jar fills the gap.)
    cookie: replay?.cookie ?? await cookiesFor(url),
    requestHeaders: replay?.headers,
    filename: baseName(filename),
    fileSize: fileSize > 0 ? fileSize : undefined,
    referer: referrer || undefined,
    userAgent: navigator.userAgent,
    mimeType: mime || undefined,
  };
  try {
    // text/plain keeps it a "simple" request: no CORS preflight to worry about.
    const res = await admFetch("/download", { method: "POST", body: JSON.stringify(body), headers: { "Content-Type": "text/plain" } }, 5000);
    return res.ok;
  } catch {
    return false;
  }
}

/**
 * Sends several links to ADM as one group ("Grab all links"): ADM shows one window for all of them. Links are
 * grouped by site so each group carries the cookies for its own site. Resolves with how many links ADM accepted.
 */
export async function sendBatch(links, { referrer } = {}) {
  if (!(await ensureAdm())) return 0;
  const byOrigin = new Map();
  for (const l of links) {
    try { const o = new URL(l.url).origin; byOrigin.set(o, [...(byOrigin.get(o) || []), l]); } catch { /* skip */ }
  }
  let accepted = 0;
  for (const [origin, group] of byOrigin) {
    const body = {
      items: group.map((l) => ({ url: l.url, filename: l.filename, fileSize: l.size > 0 ? l.size : undefined })),
      referer: referrer || undefined,
      userAgent: navigator.userAgent,
      cookie: await cookiesFor(origin + "/"),
    };
    try {
      const res = await admFetch("/batch", { method: "POST", body: JSON.stringify(body), headers: { "Content-Type": "text/plain" } }, 8000);
      if (res.ok) accepted += group.length;
    } catch { /* ADM went away: this group is not accepted */ }
  }
  return accepted;
}
