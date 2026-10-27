// Finding the videos and audio a page loads, from the response headers the browser reports. Plain functions: no
// browser APIs here, so they are easy to test.
//
// Only files that can be downloaded as they are are listed. Streams made of many small pieces (HLS `.m3u8`, DASH
// `.mpd`, `.ts`/`.m4s` segments) are skipped: ADM cannot assemble those yet.

export const MIN_MEDIA_BYTES = 256 * 1024; // smaller than this is a thumbnail, an ad or a sound effect
export const MAX_PER_TAB = 40;

const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|wmv|flv|mpg|mpeg|3gp|mp3|m4a|aac|ogg|oga|opus|flac|wav)$/i;
const AUDIO_EXT = /\.(mp3|m4a|aac|ogg|oga|opus|flac|wav)$/i;
const STREAM_EXT = /\.(m3u8|mpd|ts|m4s)$/i;

const MIME_LABEL = {
  "video/mp4": "MP4", "video/webm": "WEBM", "video/x-matroska": "MKV", "video/quicktime": "MOV", "video/x-msvideo": "AVI",
  "video/mpeg": "MPG", "video/3gpp": "3GP", "video/x-flv": "FLV", "audio/mpeg": "MP3", "audio/mp3": "MP3", "audio/mp4": "M4A",
  "audio/aac": "AAC", "audio/ogg": "OGG", "audio/wav": "WAV", "audio/x-wav": "WAV", "audio/flac": "FLAC", "audio/webm": "WEBM",
};

const pathOf = (url) => { try { return new URL(url).pathname; } catch { return ""; } };
const header = (headers, name) => headers?.find((h) => h.name.toLowerCase() === name)?.value;

/**
 * `details` is what `webRequest.onHeadersReceived` reports. Returns `{ url, kind, ext, size, mime }` for a media
 * file worth listing, or null.
 */
export function classify(details) {
  if (details.statusCode !== 200 && details.statusCode !== 206) return null;
  if (!/^https?:/i.test(details.url || "")) return null;
  const path = pathOf(details.url);
  if (STREAM_EXT.test(path)) return null;
  const mime = (header(details.responseHeaders, "content-type") || "").split(";")[0].trim().toLowerCase();
  if (/mpegurl|dash\+xml/.test(mime)) return null;

  let kind = null;
  if (mime.startsWith("video/")) kind = "video";
  else if (mime.startsWith("audio/")) kind = "audio";
  else if (/^(application|binary)\/octet-stream$/.test(mime) && MEDIA_EXT.test(path)) kind = AUDIO_EXT.test(path) ? "audio" : "video";
  if (!kind) return null;

  // The whole file's size: from `Content-Range: bytes 0-99/12345` for a partial reply, `Content-Length` otherwise
  // (for a partial reply that is only the size of the piece, so it is not used).
  let size = null;
  const total = /\/(\d+)\s*$/.exec(header(details.responseHeaders, "content-range") || "");
  if (total) size = Number(total[1]);
  else if (details.statusCode === 200 && Number(header(details.responseHeaders, "content-length")) > 0) size = Number(header(details.responseHeaders, "content-length"));
  if (size !== null && size < MIN_MEDIA_BYTES) return null;

  const fromPath = MEDIA_EXT.test(path) ? /\.([a-z0-9]+)$/i.exec(path)[1] : null;
  const ext = (fromPath || MIME_LABEL[mime] || mime.split("/")[1] || "FILE").toUpperCase();
  return { url: details.url.split("#")[0], kind, ext, size, mime };
}

/** Adds a found file to a tab's list: newest first, each address once, at most `MAX_PER_TAB`. */
export function addEntry(list, entry, now = Date.now()) {
  const same = list.find((e) => e.url === entry.url);
  if (same) {
    // Seen again (players fetch a file in many pieces): keep one row, learn the size if it was unknown.
    if (same.size == null && entry.size != null) return list.map((e) => (e === same ? { ...e, size: entry.size } : e));
    return list;
  }
  return [{ ...entry, t: now }, ...list].slice(0, MAX_PER_TAB);
}

export const mediaKey = (tabId) => `media:${tabId}`;

/** A file name for a found video: the page's title (what the user sees), else the address's own name. */
export function mediaFileName(entry, pageTitle) {
  const own = decodeURIComponent((pathOf(entry.url).split("/").pop() || "").trim());
  const title = (pageTitle || "").replace(/[\\/:*?"<>|\u0000-\u001f]+/g, " ").replace(/\s+/g, " ").trim().slice(0, 120);
  const ext = entry.ext.toLowerCase();
  if (title) return `${title}.${ext}`;
  return own && own.includes(".") ? own : `${own || "video"}.${ext}`;
}

export function formatSize(n) {
  if (n == null) return "";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return (i === 0 ? n.toFixed(0) : n >= 100 ? n.toFixed(0) : n.toFixed(1)) + " " + units[i];
}
