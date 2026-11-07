#!/usr/bin/env python3
"""Serve the Canvas Web build on one origin, with the native bridge proxied.

The page talks to the bridge over same-origin `/v1/*`. Serving `dist/` behind a
reverse proxy keeps that true whatever the page's public URL is: over loopback,
over a tailnet name, or behind TLS. Calling `127.0.0.1:8090` directly from the
browser cannot work there — it names the *viewer's* machine, and an https page
may not fetch an http endpoint at all.

    python3 serve-local.py [--port 8092] [--bridge http://127.0.0.1:8090]

Nothing is authenticated here; this is a local demo host, not a deployment.
"""
import argparse
import http.server
import socketserver
import sys
import urllib.error
import urllib.request
from pathlib import Path

BRIDGE = "http://127.0.0.1:8090"
DIST = Path(__file__).resolve().parent / "dist"


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(DIST), **kwargs)

    def do_GET(self):
        if self.path.startswith("/v1/"):
            return self.proxy()
        return super().do_GET()

    def do_POST(self):
        if self.path.startswith("/v1/"):
            return self.proxy()
        self.send_error(405)

    def proxy(self):
        url = BRIDGE + self.path
        try:
            with urllib.request.urlopen(url, timeout=10) as upstream:
                body = upstream.read()
                self.send_response(upstream.status)
                self.send_header("Content-Type", upstream.headers.get_content_type())
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        except urllib.error.HTTPError as error:
            body = error.read()
            self.send_response(error.code)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except OSError:
            # The bridge is not running. Say so; the page falls back to its WASM
            # renderer rather than showing an empty frame.
            self.send_error(503, "native bridge unreachable")

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))


def main():
    global BRIDGE
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8092)
    parser.add_argument("--bridge", default=BRIDGE)
    args = parser.parse_args()

    BRIDGE = args.bridge
    if not DIST.is_dir():
        sys.exit(f"no build at {DIST} — run `trunk build` first")

    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", args.port), Handler) as httpd:
        print(f"serving {DIST} on http://127.0.0.1:{args.port}  (bridge: {BRIDGE})")
        httpd.serve_forever()


if __name__ == "__main__":
    main()
