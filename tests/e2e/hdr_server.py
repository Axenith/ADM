import http.server, sys, json, os
SIZE = int(os.environ.get("HDR_SIZE", 3 * 1024 * 1024))
def byte(i): return (i * 31 + 7) & 0xFF
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_GET(self):
        rng = self.headers.get("Range")
        with open(os.environ.get("HDR_LOG", "/tmp/adm-hdr.log"), "a") as f:
            f.write(json.dumps({"path": self.path, "cookie": self.headers.get("Cookie"), "referer": self.headers.get("Referer"),
                                "ua": self.headers.get("User-Agent"), "range": rng, "inm": self.headers.get("If-None-Match"), "ae": self.headers.get("Accept-Encoding"), "custom": self.headers.get("X-Custom"), "auth": self.headers.get("Authorization")}) + "\n")
        start, end = 0, SIZE - 1
        if rng and rng.startswith("bytes="):
            a, b = rng[6:].split("-"); start = int(a); end = int(b) if b else SIZE - 1
        data = bytes(byte(i) for i in range(start, end + 1))
        self.send_response(206 if rng else 200)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Last-Modified", "Wed, 21 Oct 2015 07:28:00 GMT")
        if rng: self.send_header("Content-Range", f"bytes {start}-{end}/{SIZE}")
        self.end_headers()
        if self.path.startswith("/slow/"):  # a slow server: 64 KiB, then a pause, so a download takes a few seconds
            import time
            for i in range(0, len(data), 65536):
                self.wfile.write(data[i:i + 65536]); self.wfile.flush(); time.sleep(0.4)
        else:
            self.wfile.write(data)
http.server.ThreadingHTTPServer(("127.0.0.1", 18090), H).serve_forever()
