// "Grab all links for this page": collecting a page's links, and sorting them by what they probably are.

/**
 * Runs INSIDE the page (it is handed to `scripting.executeScript`, so it must not use anything from outside it).
 * Returns every http(s) address it can find, each with where it came from:
 *   "link"      something the page links to (anchors, `<a download>`)
 *   "resource"  something the page itself uses (video/audio, images with their largest `srcset` candidate, posters,
 *               preloads, CSS backgrounds) plus everything the browser fetched for it (Resource Timing)
 */
export function collectLinks() {
  const found = new Map();
  const add = (href, text, source) => {
    try {
      const u = new URL(href, location.href);
      if (u.protocol !== "http:" && u.protocol !== "https:") return;
      const old = found.get(u.href);
      if (old) { if (source === "link") old.source = "link"; if (!old.text && text) old.text = clean(text); return; }
      found.set(u.href, { url: u.href, text: clean(text), source });
    } catch { /* not a link */ }
  };
  const clean = (t) => (t || "").replace(/\s+/g, " ").trim().slice(0, 120);
  // The biggest picture a `srcset` offers ("a.jpg 480w, b.jpg 1080w" or "a.jpg 1x, b.jpg 2x").
  const largest = (srcset) => {
    let best = null, bestScore = -1;
    for (const part of (srcset || "").split(/,\s+(?=\S)/)) {
      const [u, d] = part.trim().split(/\s+/);
      const score = d ? parseFloat(d) || 0 : 1;
      if (u && score > bestScore) { best = u; bestScore = score; }
    }
    return best;
  };
  const roots = [document];
  const scan = (root) => {
    root.querySelectorAll("a[href]").forEach((a) => add(a.href, a.getAttribute("download") || a.textContent || a.title, "link"));
    root.querySelectorAll("video[src], audio[src], source[src], track[src], embed[src]").forEach((el) => add(el.src, el.getAttribute("title"), "resource"));
    root.querySelectorAll("video[poster]").forEach((el) => add(el.poster, "", "resource"));
    root.querySelectorAll("img").forEach((el) => {
      const best = largest(el.getAttribute("srcset"));
      if (best) add(best, el.alt, "resource");
      if (el.currentSrc || el.src) add(el.currentSrc || el.src, el.alt, "resource");
    });
    root.querySelectorAll("source[srcset]").forEach((el) => { const b = largest(el.getAttribute("srcset")); if (b) add(b, "", "resource"); });
    root.querySelectorAll('link[rel~="preload"][href], link[rel~="icon"][href]').forEach((el) => add(el.href, "", "resource"));
    root.querySelectorAll("[style*='url(']").forEach((el) => {
      for (const m of el.getAttribute("style").matchAll(/url\((['"]?)(.*?)\1\)/g)) add(m[2], "", "resource");
    });
    // Open shadow roots hold links too (web components).
    root.querySelectorAll("*").forEach((el) => { if (el.shadowRoot) roots.push(el.shadowRoot); });
  };
  for (let i = 0; i < roots.length && i < 200; i++) scan(roots[i]);
  for (const e of performance.getEntriesByType("resource")) add(e.name, "", "resource");
  return { links: [...found.values()].slice(0, 5000), title: document.title, url: location.href };
}

const KINDS = {
  video: "mp4 m4v webm mkv mov avi wmv flv mpg mpeg 3gp ts",
  audio: "mp3 m4a aac ogg oga opus flac wav",
  archive: "zip rar 7z tar gz xz bz2 zst tgz iso img dmg cab",
  document: "pdf doc docx xls xlsx ppt pptx epub odt ods odp txt csv",
  program: "exe msi deb rpm appimage apk xapk pkg msix jar",
  image: "jpg jpeg png gif webp svg bmp avif heic",
};

/** `{ kind, ext, name }`: kind is "page" for an address with no recognisable file extension. */
export function classifyLink(url) {
  let path = "";
  try { path = new URL(url).pathname; } catch { /* leave empty */ }
  const last = decodeURIComponent(path.split("/").pop() || "");
  const ext = /\.([a-z0-9]{1,6})$/i.exec(last)?.[1]?.toLowerCase() || "";
  for (const [kind, list] of Object.entries(KINDS)) if (list.split(" ").includes(ext)) return { kind, ext, name: last };
  return { kind: ext && !/^(html?|php|aspx?|jsp|cgi)$/.test(ext) ? "other" : "page", ext, name: last };
}

/** Hosts and paths that are almost always ads, counters and trackers rather than something to download. */
const NOISE_HOST = /(^|\.)(doubleclick|googlesyndication|googleadservices|google-analytics|googletagmanager|adservice\.google|scorecardresearch|facebook\.net|hotjar|quantserve|taboola|outbrain|adnxs|criteo)\./i;
const NOISE_PATH = /(\/(pixel|beacon|collect|track(ing)?|analytics|favicon)\b|[?&](utm_|fbclid|gclid)|1x1\.|spacer\.)/i;

/** Ads, trackers, favicons and other things nobody means to download. */
export function isNoise(url) {
  try {
    const u = new URL(url);
    return NOISE_HOST.test(u.hostname) || NOISE_PATH.test(u.pathname + u.search);
  } catch { return false; }
}

/**
 * Turns what the user typed in the filter box into a test on `{ url, text, name }`. Three forms, like XDM's:
 *   words           every word must appear (in the address or the name)
 *   *.jpg  img?.*   a wildcard pattern, matched against the file name or the whole address
 *   /regex/i        a regular expression (a broken one matches nothing)
 */
export function makeMatcher(query) {
  const q = (query || "").trim();
  if (!q) return () => true;
  const re = /^\/(.+)\/([a-z]*)$/.exec(q);
  if (re) {
    try { const rx = new RegExp(re[1], re[2].includes("i") ? re[2] : re[2] + "i"); return (l) => rx.test(l.url) || rx.test(l.name || "") || rx.test(l.text || ""); }
    catch { return () => false; }
  }
  if (/[*?]/.test(q)) {
    const rx = new RegExp("^" + q.split(/([*?])/).map((p) => p === "*" ? ".*" : p === "?" ? "." : p.replace(/[.+^${}()|[\]\\]/g, "\\$&")).join("") + "$", "i");
    return (l) => rx.test(l.name || "") || rx.test(l.url);
  }
  const words = q.toLowerCase().split(/\s+/);
  return (l) => { const hay = (l.url + " " + (l.text || "")).toLowerCase(); return words.every((w) => hay.includes(w)); };
}
