// "Grab all links for this page": collecting a page's links, and sorting them by what they probably are.

/**
 * Runs INSIDE the page (it is handed to `scripting.executeScript`, so it must not use anything from outside it).
 * Returns every http(s) link it can find: anchors, and the sources of video/audio elements.
 */
export function collectLinks() {
  const found = new Map();
  const add = (href, text) => {
    try {
      const u = new URL(href, location.href);
      if ((u.protocol === "http:" || u.protocol === "https:") && !found.has(u.href)) {
        found.set(u.href, { url: u.href, text: (text || "").replace(/\s+/g, " ").trim().slice(0, 120) });
      }
    } catch { /* not a link */ }
  };
  document.querySelectorAll("a[href]").forEach((a) => add(a.href, a.textContent || a.title || a.getAttribute("download") || ""));
  document.querySelectorAll("video[src], audio[src], source[src]").forEach((el) => add(el.src, el.getAttribute("title") || ""));
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
