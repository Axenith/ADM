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

export const DEFAULTS = { enabled: true, minSizeMB: 0, monitoring: true };

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
    return body?.app === "adm" ? body : null; // not any other program that happens to answer on the port
  } catch {
    return null;
  }
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
 * Gives ADM the link plus what the browser knows: the cookies the site would see (if allowed), the page it came
 * from, the name the browser chose. Resolves true only if ADM accepted it.
 */
export async function handOver({ url, referrer, filename, fileSize, mime }) {
  const body = {
    url,
    cookie: await cookiesFor(url),
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
