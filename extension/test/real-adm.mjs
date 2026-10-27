// Integration check: the extension's real code against a RUNNING ADM (start it first).
//   node extension/test/real-adm.mjs
// A browser adds `Origin: chrome-extension://...` to extension requests; Node does not, so it is added here.
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";

const SIZE = 3 * 1024 * 1024;
const byte = (i) => (i * 31 + 7) & 0xff;
const seen = [];
const file = http.createServer((req, res) => {
  seen.push({ cookie: req.headers.cookie, referer: req.headers.referer, range: req.headers.range, ua: req.headers["user-agent"] });
  const m = /bytes=(\d+)-(\d*)/.exec(req.headers.range || "");
  const start = m ? Number(m[1]) : 0, end = m && m[2] ? Number(m[2]) : SIZE - 1;
  const buf = Buffer.alloc(end - start + 1);
  for (let i = 0; i < buf.length; i++) buf[i] = byte(start + i);
  res.writeHead(m ? 206 : 200, { "Content-Length": buf.length, "Accept-Ranges": "bytes", "Last-Modified": "Wed, 21 Oct 2015 07:28:00 GMT", ...(m && { "Content-Range": `bytes ${start}-${end}/${SIZE}` }) });
  res.end(buf);
});
await new Promise((r) => file.listen(0, "127.0.0.1", r));
const url = `http://127.0.0.1:${file.address().port}/real-adm-test.bin`;

const realFetch = globalThis.fetch;
globalThis.fetch = (u, init = {}) => realFetch(u, { ...init, headers: { ...init.headers, Origin: "chrome-extension://abcdefghijklmnop" } });
globalThis.chrome = { cookies: { getAll: async () => [{ name: "session", value: "from-the-browser" }] }, storage: { local: { get: async (d) => d } } };

const { isAlive, handOver } = await import("../adm.js");
assert.equal(await isAlive(), true, "ADM is running and recognises the extension");
console.log("PASS  extension sees a running ADM");

const ok = await handOver({ url, referrer: "https://site.example/page", filename: "/x/y/real-adm-test.bin", fileSize: SIZE, mime: "application/octet-stream" });
assert.equal(ok, true, "ADM accepted the download");
console.log("PASS  ADM accepted the hand-over");

const dest = path.join(os.homedir(), "Downloads", "real-adm-test.bin");
for (let i = 0; i < 60 && !(fs.existsSync(dest) && fs.statSync(dest).size === SIZE); i++) await new Promise((r) => setTimeout(r, 500));
assert.ok(fs.existsSync(dest) && fs.statSync(dest).size === SIZE, "the file arrived in ~/Downloads");
const data = fs.readFileSync(dest);
for (let i = 0; i < SIZE; i += 997) assert.equal(data[i], byte(i), `byte ${i}`);
console.log("PASS  file downloaded byte-exact under the browser's name");
assert.ok(seen.length > 0 && seen.every((r) => r.cookie === "session=from-the-browser"), "cookie reached the origin");
assert.ok(seen.every((r) => r.referer === "https://site.example/page"), "referer reached the origin");
console.log(`PASS  cookie + referer reached the origin server (${seen.length} requests, ${seen.filter((r) => r.range).length} ranged)`);
file.close();
