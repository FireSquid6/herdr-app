#!/usr/bin/env python3
"""Synthetic browser gate fixture, loopback only. No privileged browser API.

Prints its ephemeral origin as JSON. Ctrl-C stops this process. State is volatile;
uploads are bounded, hashed, and never written to disk. No real credentials.
"""

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
from urllib.parse import unquote, urlsplit


MAX_UPLOAD = 1024 * 1024
DOWNLOAD = b"Herdr workspace browser synthetic download\n"
PAGE = Path(__file__).with_name("page.html").read_bytes()
POPUP = b"""<!doctype html><title>Opener test</title><h1>Popup with original opener</h1>
<output id="opener"></output><script>
document.querySelector('#opener').textContent = window.opener ? 'opener retained' : 'NO OPENER';
if (window.opener) window.opener.postMessage('fixture-popup', location.origin);
</script>"""


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, port=0):
        super().__init__(("127.0.0.1", port), Handler)
        self.lock = threading.Lock()
        self.requests = 0
        self.uploads = []


class Handler(BaseHTTPRequestHandler):
    server_version = "HerdrSyntheticFixture/1"

    def log_message(self, *_args):
        pass  # Never echo URL queries or file names into logs.

    def reply(self, body, kind="text/plain; charset=utf-8", status=200, headers=None):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def json_reply(self, value, status=200):
        self.reply(json.dumps(value).encode(), "application/json", status)

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/":
            self.reply(PAGE, "text/html; charset=utf-8")
        elif path == "/popup":
            self.reply(POPUP, "text/html; charset=utf-8")
        elif path == "/request":
            with self.server.lock:
                self.server.requests += 1
                count = self.server.requests
            self.json_reply({"fixture_request": count})
        elif path == "/observed":
            with self.server.lock:
                observed = {"requests": self.server.requests, "uploads": list(self.server.uploads)}
            self.json_reply(observed)
        elif path == "/download":
            self.reply(DOWNLOAD, headers={"Content-Disposition": 'attachment; filename="fixture-download.txt"'})
        elif path == "/cookie":
            self.reply(b"synthetic cookie set", headers={"Set-Cookie": "fixture=workspace-test; Path=/; SameSite=Lax"})
        elif path == "/favicon.ico":
            self.reply(b"", status=204)
        else:
            self.reply(b"not found", status=404)

    def do_POST(self):
        if urlsplit(self.path).path != "/upload":
            self.close_connection = True
            self.reply(b"not found", status=404)
            return
        length = self.headers.get("Content-Length", "")
        if not length.isascii() or not length.isdecimal():
            self.close_connection = True
            self.reply(b"Content-Length required", status=411)
            return
        length = int(length)
        if length > MAX_UPLOAD:
            self.close_connection = True
            self.reply(b"fixture upload limit is 1 MiB", status=413)
            return
        self.connection.settimeout(5)
        body = self.rfile.read(length)
        if len(body) != length:
            self.close_connection = True
            self.reply(b"incomplete upload", status=400)
            return
        # Browser-supplied file names are display data, never filesystem paths.
        name = unquote(self.headers.get("X-Fixture-Name", "unnamed"))[:256]
        record = {"name": name, "size": len(body), "sha256": hashlib.sha256(body).hexdigest()}
        with self.server.lock:
            # Keep fixture state bounded, even if a test retries indefinitely.
            self.server.uploads = (self.server.uploads + [record])[-16:]
        self.json_reply(record)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    server = FixtureServer(args.port)
    print(json.dumps({"origin": f"http://127.0.0.1:{server.server_port}", "synthetic": True}), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
