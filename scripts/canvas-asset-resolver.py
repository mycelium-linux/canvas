#!/usr/bin/env python3
"""Local asset resolver: content-addressed store for Canvas.

Holds credentials and resolves an Alfalfa resource_id (or fpl-storage blob
hash) into local bytes at a content-addressed path. Canvas opens files from
the store via AssetRef{sha256} — it never sees a URL.

  canvas-asset-resolver.py --storage http://127.0.0.1:8799 --port 8798

Endpoints:
  GET  /resolve/{id}    Resolve a resource_id or blob hash to a local path.
                         Returns {"sha256","media_type","byte_length","path"}
  GET  /assets/{hash}   Serve bytes for a given sha256 (or 404).
  POST /sync            Re-validate a resource_id and update its position.
"""
import hashlib
import json
import os
import pathlib
import time
import urllib.request
import urllib.error
from http.server import HTTPServer, BaseHTTPRequestHandler

ASSET_ROOT = pathlib.Path(
    os.environ.get("CANVAS_ASSET_ROOT") or
    os.path.expanduser("~/.canvas/assets")
)
ASSET_ROOT.mkdir(parents=True, exist_ok=True)


def sha256_file(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def resolve_resource(resource_id: str, storage_url: str) -> dict:
    # Try fpl-storage blob fetch first
    url = f"{storage_url}/blob/{resource_id}"
    try:
        req = urllib.request.Request(url, headers={"Accept": "*/*"})
        with urllib.request.urlopen(req, timeout=30) as resp:
            media_type = resp.headers.get("Content-Type", "application/octet-stream")
            byte_length = int(resp.headers.get("Content-Length", 0))
            tmp = ASSET_ROOT / f".tmp-{resource_id}"
            with open(tmp, "wb") as f:
                while True:
                    chunk = resp.read(65536)
                    if not chunk:
                        break
                    f.write(chunk)
            actual_length = tmp.stat().st_size
            actual_sha = sha256_file(tmp)
            final = ASSET_ROOT / actual_sha
            if not final.exists():
                tmp.rename(final)
            else:
                tmp.unlink()
            return {
                "sha256": actual_sha,
                "media_type": media_type,
                "byte_length": actual_length,
                "path": str(final),
                "cached": False,
            }
    except urllib.error.HTTPError as exc:
        if exc.code != 404:
            raise
        raise FileNotFoundError(f"Resource not found: {resource_id}")
    except urllib.error.URLError as exc:
        raise ConnectionError(f"Cannot reach storage at {storage_url}: {exc}")


def serve_cached(sha256: str) -> pathlib.Path:
    path = ASSET_ROOT / sha256
    if not path.exists():
        raise FileNotFoundError(f"Asset not in store: {sha256}")
    return path


class Handler(BaseHTTPRequestHandler):
    storage_url = "http://127.0.0.1:8799"

    def do_GET(self):
        path = self.path.rstrip("/")
        if path.startswith("/resolve/"):
            resource_id = path[len("/resolve/"):]
            try:
                result = resolve_resource(resource_id, self.storage_url)
                self._json(200, result)
            except FileNotFoundError as exc:
                self._json(404, {"error": str(exc)})
            except Exception as exc:
                self._json(502, {"error": str(exc)})
        elif path.startswith("/assets/"):
            sha = path[len("/assets/"):]
            try:
                file = serve_cached(sha)
                data = file.read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            except FileNotFoundError:
                self._json(404, {"error": "asset not found"})
        elif path == "/health":
            self._json(200, {"status": "ok", "asset_count": len(list(ASSET_ROOT.glob("*")))})
        else:
            self._json(404, {"error": "unknown endpoint"})

    def do_POST(self):
        if self.path.rstrip("/") == "/sync":
            content = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            try:
                payload = json.loads(content)
                resource_id = payload.get("resource_id", "")
                result = resolve_resource(resource_id, self.storage_url)
                self._json(200, result)
            except Exception as exc:
                self._json(502, {"error": str(exc)})
        else:
            self._json(404, {"error": "unknown endpoint"})

    def _json(self, code: int, body: dict):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        pass


def main():
    import argparse
    parser = argparse.ArgumentParser(description="Canvas asset resolver")
    parser.add_argument("--storage", default="http://127.0.0.1:8799",
                        help="fpl-storage URL")
    parser.add_argument("--port", type=int, default=8798, help="listen port")
    args = parser.parse_args()
    Handler.storage_url = args.storage
    server = HTTPServer(("127.0.0.1", args.port), Handler)
    print(f"canvas-asset-resolver: storage={args.storage} on :{args.port}", flush=True)
    print(f"canvas-asset-resolver: asset root = {ASSET_ROOT}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
