// The pure logic: what counts as a video, how big it is, naming, and sorting a page's links.
import test from "node:test";
import assert from "node:assert/strict";
import { MAX_PER_TAB, MIN_MEDIA_BYTES, addEntry, classify, formatSize, isStreamPiece, mediaFileName } from "../media.js";
import { classifyLink } from "../grab-lib.js";
import { capturedByRules, toReplay } from "../adm.js";
import { isNoise, makeMatcher } from "../grab-lib.js";

const resp = (url, mime, extra = {}) => ({
  url, statusCode: 200, tabId: 1,
  responseHeaders: [{ name: "Content-Type", value: mime }, { name: "Content-Length", value: String(extra.length ?? 5_000_000) }, ...(extra.headers || [])],
  ...extra.over,
});

test("videos and audio are found by their content type", () => {
  assert.deepEqual(classify(resp("https://c.example/v/clip.mp4", "video/mp4")), { url: "https://c.example/v/clip.mp4", kind: "video", ext: "MP4", size: 5_000_000, mime: "video/mp4" });
  assert.equal(classify(resp("https://c.example/a/song", "audio/mpeg")).ext, "MP3", "no extension in the address: the type names it");
  assert.equal(classify(resp("https://c.example/a/song", "audio/mpeg")).kind, "audio");
  assert.equal(classify(resp("https://c.example/x", "video/webm; codecs=vp9")).ext, "WEBM", "parameters after the type are ignored");
});

test("a generic binary type counts only when the address says it is media", () => {
  assert.equal(classify(resp("https://c.example/f/movie.mkv", "application/octet-stream"))?.ext, "MKV");
  assert.equal(classify(resp("https://c.example/f/track.flac", "application/octet-stream"))?.kind, "audio");
  assert.equal(classify(resp("https://c.example/f/archive.zip", "application/octet-stream")), null);
  assert.equal(classify(resp("https://c.example/download?id=1", "application/octet-stream")), null);
});

test("things that are not a plain media file are skipped", () => {
  assert.equal(classify(resp("https://c.example/live/index.m3u8", "application/vnd.apple.mpegurl")), null, "HLS playlist");
  assert.equal(classify(resp("https://c.example/live/seg1.ts", "video/mp2t")), null, "HLS segment");
  assert.equal(classify(resp("https://c.example/v/manifest.mpd", "application/dash+xml")), null, "DASH manifest");
  assert.equal(classify(resp("https://c.example/v/chunk.m4s", "video/iso.segment")), null, "DASH segment");
  assert.equal(classify(resp("https://c.example/p.png", "image/png")), null);
  assert.equal(classify(resp("https://c.example/page", "text/html")), null);
  assert.equal(classify(resp("blob:https://c.example/1", "video/mp4")), null);
  assert.equal(classify(resp("https://c.example/v.mp4", "video/mp4", { over: { statusCode: 404 } })), null);
  assert.equal(classify(resp("https://c.example/v.mp4", "video/mp4", { over: { statusCode: 304 } })), null);
});

test("tiny files are thumbnails or ads, not videos", () => {
  assert.equal(classify(resp("https://c.example/ad.mp4", "video/mp4", { length: MIN_MEDIA_BYTES - 1 })), null);
  assert.ok(classify(resp("https://c.example/ok.mp4", "video/mp4", { length: MIN_MEDIA_BYTES })));
  const unknown = classify({ url: "https://c.example/s.mp4", statusCode: 200, responseHeaders: [{ name: "content-type", value: "video/mp4" }] });
  assert.equal(unknown.size, null, "no length: still listed, size unknown");
});

test("the whole file's size comes from Content-Range, not the piece that was sent", () => {
  const piece = resp("https://c.example/big.mp4", "video/mp4", { length: 1_000_000, over: { statusCode: 206 }, headers: [{ name: "Content-Range", value: "bytes 0-999999/46342963" }] });
  assert.equal(classify(piece).size, 46_342_963);
  const unknownTotal = resp("https://c.example/big.mp4", "video/mp4", { length: 1_000_000, over: { statusCode: 206 }, headers: [{ name: "Content-Range", value: "bytes 0-999999/*" }] });
  assert.equal(classify(unknownTotal).size, null, "a piece's own length is never mistaken for the file's");
});

test("the same file seen many times is listed once, and learns its size", () => {
  const a = { url: "https://c.example/a.mp4", kind: "video", ext: "MP4", size: null, mime: "video/mp4" };
  let list = addEntry([], a, 1);
  assert.equal(list.length, 1);
  assert.equal(addEntry(list, { ...a }, 2), list, "nothing new: the very same list");
  list = addEntry(list, { ...a, size: 900_000 }, 3);
  assert.equal(list.length, 1);
  assert.equal(list[0].size, 900_000);
  list = addEntry(list, { ...a, url: "https://c.example/b.mp4" }, 4);
  assert.deepEqual(list.map((e) => e.url), ["https://c.example/b.mp4", "https://c.example/a.mp4"], "newest first");
  let many = [];
  for (let i = 0; i < MAX_PER_TAB + 10; i++) many = addEntry(many, { ...a, url: `https://c.example/${i}.mp4` }, i);
  assert.equal(many.length, MAX_PER_TAB, "capped");
  assert.equal(many[0].url, `https://c.example/${MAX_PER_TAB + 9}.mp4`, "the newest are kept");
});

test("a download is named after the page, like what the user sees", () => {
  const e = { url: "https://c.example/v/abc123.mp4", ext: "MP4" };
  assert.equal(mediaFileName(e, "Scenic Palm Trees on São Miguel Beach"), "Scenic Palm Trees on São Miguel Beach.mp4");
});

test("file names are made safe, and fall back to the address", () => {
  const e = { url: "https://c.example/v/abc123.mp4", ext: "MP4" };
  assert.equal(mediaFileName(e, 'A/B: "C"?*'), "A B C.mp4");
  assert.equal(mediaFileName(e, ""), "abc123.mp4");
  assert.equal(mediaFileName({ url: "https://c.example/watch", ext: "WEBM" }, ""), "watch.webm");
  assert.ok(mediaFileName(e, "x".repeat(500)).length <= 124, "long titles are cut");
});

test("sizes read like in ADM", () => {
  assert.equal(formatSize(46_342_963), "44.2 MB");
  assert.equal(formatSize(2_900_000), "2.8 MB");
  assert.equal(formatSize(null), "");
  assert.equal(formatSize(1_500_000_000), "1.4 GB");
});

test("a page's links are sorted by what they probably are", () => {
  const k = (u) => classifyLink(u).kind;
  assert.equal(k("https://x.io/files/Movie.MP4?dl=1"), "video");
  assert.equal(k("https://x.io/a/song.flac"), "audio");
  assert.equal(k("https://x.io/b/backup.tar.gz"), "archive");
  assert.equal(k("https://x.io/c/paper.pdf"), "document");
  assert.equal(k("https://x.io/d/setup.exe"), "program");
  assert.equal(k("https://x.io/e/photo.jpg"), "image");
  assert.equal(k("https://x.io/about"), "page");
  assert.equal(k("https://x.io/index.html"), "page");
  assert.equal(k("https://x.io/"), "page");
  assert.equal(k("https://x.io/data.bin"), "other", "an unknown extension is still a file");
  assert.equal(classifyLink("https://x.io/My%20File.zip").name, "My File.zip");
  assert.equal(classifyLink("not a url").kind, "page");
});

test("pieces of a stream are never listed as videos", () => {
  // What a YouTube page's player fetches: slices of one stream, from video servers.
  assert.equal(isStreamPiece("https://rr3---sn-abc.googlevideo.com/videoplayback?expire=1&itag=22&range=0-1048575&rn=1&rbuf=2000"), true);
  assert.equal(isStreamPiece("https://www.youtube.com/watch?v=abc"), true);
  assert.equal(isStreamPiece("https://youtu.be/abc"), true);
  assert.equal(isStreamPiece("https://cdn.example/v/clip.mp4?bytestart=100&byteend=2000"), true);
  assert.equal(isStreamPiece("https://cdn.example/v/clip.mp4?sq=12"), true);
  assert.equal(isStreamPiece("https://cdn.example/v/seg-12.mp4"), true);
  assert.equal(isStreamPiece("https://cdn.example/v/chunk_003.m4s"), true);
  // Ordinary files, including ones with token parameters, are not slices.
  assert.equal(isStreamPiece("https://cdn.example/v/clip.mp4?token=abc&expires=17"), false);
  assert.equal(isStreamPiece("https://example.com/movies/segment.mp4"), false, "a name that merely contains 'segment'");
  assert.equal(isStreamPiece("https://notyoutube.com/a.mp4"), false, "a different site");
  assert.equal(classify({ url: "https://rr3.googlevideo.com/videoplayback?range=0-999999", statusCode: 206, responseHeaders: [{ name: "Content-Type", value: "video/mp4" }, { name: "Content-Range", value: "bytes 0-999999/9999999" }] }), null);
});

test("capture rules: listed types, or big files", () => {
  const rules = { fileExts: ["ZIP", "MP4"], captureOverMB: 10 };
  assert.equal(capturedByRules({ url: "https://x/a", filename: "/d/A.Zip" }, rules), true, "case does not matter");
  assert.equal(capturedByRules({ url: "https://x/a.mp4" }, rules), true, "the name comes from the address when the browser gave none");
  assert.equal(capturedByRules({ url: "https://x/a.txt", filename: "a.txt", fileSize: 1000 }, rules), false);
  assert.equal(capturedByRules({ url: "https://x/a.txt", filename: "a.txt", fileSize: 10 * 1048576 }, rules), true, "exactly the limit counts");
  assert.equal(capturedByRules({ url: "https://x/a.txt", filename: "a.txt", fileSize: 10 * 1048576 - 1 }, rules), false);
  assert.equal(capturedByRules({ url: "https://x/a.txt", filename: "a.txt" }, {}), true, "no rules published: everything");
  assert.equal(capturedByRules({ url: "https://x/a.txt", filename: "a.txt" }, { fileExts: [] }), false, "an empty list is a rule too");
});

test("replayed headers: what a download manager needs, and the cookie apart", () => {
  const r = toReplay([{ name: "COOKIE", value: "a=1" }, { name: "Referer", value: "https://x/" }, { name: "sec-ch-ua", value: "x" }, { name: "X-Dup", value: "1" }, { name: "x-dup", value: "2" }, { name: "Host", value: "h" }]);
  assert.equal(r.cookie, "a=1");
  assert.deepEqual(r.headers, { Referer: ["https://x/"], "X-Dup": ["1"], "x-dup": ["2"] });
  assert.deepEqual(toReplay(undefined), { headers: {}, cookie: undefined });
});

test("the grab filter: words, wildcard patterns and regular expressions", () => {
  const l = (url, text = "") => ({ url, text, name: decodeURIComponent(url.split("/").pop()) });
  assert.equal(makeMatcher("")(l("https://a/x")), true);
  assert.equal(makeMatcher("holiday jpg")(l("https://a/holiday-1.jpg")), true, "every word, in any order");
  assert.equal(makeMatcher("holiday jpg")(l("https://a/holiday-1.png")), false);
  assert.equal(makeMatcher("trip")(l("https://a/x.zip", "My Trip")), true, "the link text counts, any case");
  assert.equal(makeMatcher("*.jpg")(l("https://a/b/x.JPG")), true);
  assert.equal(makeMatcher("*.jpg")(l("https://a/b/x.jpg?size=big")), false, "a pattern is the whole name");
  assert.equal(makeMatcher("img_??.png")(l("https://a/img_07.png")), true);
  assert.equal(makeMatcher("a+b.zip")(l("https://a/a+b.zip")), true, "no wildcard: plain words, nothing is a pattern");
  assert.equal(makeMatcher("a+b*")(l("https://a/a+b.zip")), true, "regex characters in a pattern are literal");
  assert.equal(makeMatcher("/\\.(png|gif)$/")(l("https://a/x.GIF")), true);
  assert.equal(makeMatcher("/(/")(l("https://a/x")), false, "a broken expression matches nothing instead of throwing");
});

test("ads, trackers and favicons are noise; real files are not", () => {
  for (const u of ["https://www.google-analytics.com/collect?v=1", "https://ad.doubleclick.net/x.gif", "https://a.example/pixel.gif", "https://a.example/favicon.ico", "https://a.example/p?utm_source=x"]) assert.equal(isNoise(u), true, u);
  for (const u of ["https://a.example/movie.mp4", "https://cdn.example/archive.zip", "not a url"]) assert.equal(isNoise(u), false, u);
});
