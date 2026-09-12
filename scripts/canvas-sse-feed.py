#!/usr/bin/env python3
"""Forward an SSE event stream to Canvas as snapshot datagrams.

Connects to a Server-Sent Events endpoint and sends each event to Canvas's
stream.sock as a canvas.widget.snapshot.v1 envelope. The payload must contain
a `widget` table (matching the target widget spec kind) and optional `data`
for source-health metadata.

Usage:
  canvas-sse-feed.py --url https://elmers.local/events/activity \\
    --owner elmers --stream attention --title "Elmer's activity"

The --widget flag injects a widget spec template so the SSE payload only
needs to provide the dynamic fields (e.g. rows, values, series).
"""
import argparse
import json
import os
import socket
import sys
import urllib.request
import urllib.error

CONTRACT = "canvas.widget.snapshot.v1"
SOCKET_PATH = os.path.join(
    os.environ.get("HOME", ""), "Library/Application Support/Dock/canvas/stream.sock"
)


def send_envelope(owner: str, stream: str, payload: dict, sequence: int):
    event = {
        "schema_version": 1,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": int(__import__("time").time() * 1000),
        "payload": payload,
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.sendto(json.dumps(event, separators=(",", ":")).encode(), SOCKET_PATH)
        sock.close()
    except Exception as exc:
        print(f"canvas-sse-feed: send failed: {exc}", file=sys.stderr)


def parse_sse_line(line: str, current: dict) -> bool:
    """Parse one SSE line. Returns True if a complete event is ready."""
    if line.startswith(":") or not line.strip():
        return False
    if line.startswith("data:"):
        data = line[5:].strip()
        if current.get("data"):
            current["data"] += "\n" + data
        else:
            current["data"] = data
    elif line.startswith("id:"):
        current["id"] = line[3:].strip()
    elif line.startswith("event:"):
        current["event"] = line[6:].strip()
    return False


def read_sse_events(url: str):
    """Yield (data_str, id_str) tuples from an SSE endpoint."""
    req = urllib.request.Request(url, headers={"Accept": "text/event-stream"})
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            current = {}
            for raw_line in resp:
                line = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
                if line == "":
                    if current.get("data"):
                        yield (current.pop("data", ""), current.pop("id", None))
                    current.clear()
                else:
                    parse_sse_line(line, current)
    except urllib.error.HTTPError as exc:
        print(f"canvas-sse-feed: SSE endpoint {url} returned {exc.code}: {exc.reason}", file=sys.stderr)
        raise
    except urllib.error.URLError as exc:
        print(f"canvas-sse-feed: cannot connect to SSE endpoint {url}: {exc}", file=sys.stderr)
        raise


def main():
    parser = argparse.ArgumentParser(description="Forward SSE events to Canvas")
    parser.add_argument("--url", required=True, help="SSE endpoint URL")
    parser.add_argument("--owner", required=True, help="Stream owner (project id)")
    parser.add_argument("--stream", required=True, help="Stream name")
    parser.add_argument("--widget", default=None,
                        help="JSON widget spec template (dynamic fields only)")
    parser.add_argument("--title", default="SSE stream", help="Widget title")
    args = parser.parse_args()

    if not os.path.exists(SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        return

    widget_template = None
    if args.widget:
        try:
            widget_template = json.loads(args.widget)
        except json.JSONDecodeError as exc:
            print(f"canvas-sse-feed: invalid --widget JSON: {exc}", file=sys.stderr)
            return

    print(f"canvas-sse-feed: connecting to {args.url}", file=sys.stderr)
    sequence = 0
    try:
        for data_str, event_id in read_sse_events(args.url):
            try:
                data = json.loads(data_str)
            except json.JSONDecodeError:
                print(f"canvas-sse-feed: non-JSON SSE payload: {data_str[:80]}", file=sys.stderr)
                continue
            sequence += 1
            payload = data if isinstance(data, dict) else {"value": data}
            if widget_template is not None:
                widget = widget_template.copy()
                if isinstance(payload, dict) and "widget" in payload:
                    widget.update(payload["widget"])
                elif isinstance(payload, dict):
                    widget["data"] = payload.get("data", {})
                payload = {"widget": widget, "data": payload.get("data", {"state": "ready", "source": "sse"})}
            send_envelope(args.owner, args.stream, payload, sequence)
    except KeyboardInterrupt:
        pass
    except Exception:
        pass


if __name__ == "__main__":
    main()
