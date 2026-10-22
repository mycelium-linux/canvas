#!/usr/bin/env python3
"""
SSE connector: relays Server-Sent Events from an HTTP endpoint into Canvas's
Unix datagram socket (stream.sock) as canvas.widget.snapshot.v1 envelopes.

This is the SSE analogue of canvas-metrics: an external process that reads
a data source the host owns, normalizes results into Canvas widget
snapshots, and publishes them on the local stream socket. Canvas itself
stays free of client code.

The event source is declared in config/views/*.json as:
  {"source": {"owner": "elmers", "id": "activity-events",
    "contract": "canvas.sse.v1", "kind": "stream"}}

A widget in that view declares:
  {"data": {"source": "sse:elmers/activity", "state": "loading"}}

The SSE connector bridges the gap: it connects to the event URL, parses
each SSE event, and publishes a canvas.widget.snapshot.v1 envelope keyed
to the matching stream identity.

Usage:
  python3 scripts/sse-connect.py \
    --socket ~/.local/state/dock/canvas/stream.sock \
    --url http://elmers:8000/events/activity \
    --owner elmers \
    --stream activity-events \
    --widget-id activity-feed
"""
import argparse
import json
import socket
import sys
import time
import urllib.request
import urllib.error
from http.client import HTTPResponse

CONTRACT = "canvas.widget.snapshot.v1"
STREAM_SCHEMA_VERSION = 1
MAX_DATAGRAM = 2048
ENVELOPE_OVERHEAD = 420


def now_ms() -> int:
    return int(time.time() * 1000)


def send_event(sock_path: str, envelope: dict) -> bool:
    """Send a single envelope to Canvas's stream socket."""
    data = json.dumps(envelope).encode()
    if len(data) > MAX_DATAGRAM:
        return False
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.connect(sock_path)
        sock.send(data)
        sock.close()
        return True
    except (FileNotFoundError, ConnectionRefusedError, OSError):
        return False


def stream_identity(owner: str, stream: str) -> tuple[str, str]:
    """Split a source id into (owner, stream) matching what Canvas expects."""
    if not owner or not stream:
        raise ValueError("owner and stream are required")
    for name in (owner, stream):
        if not name or len(name) > 64:
            raise ValueError(f"stream name must be 1-64 chars: {name}")
        if not all(c.isascii_lowercase() or c.isdigit() or c == '-' for c in name):
            raise ValueError(f"stream name must be lowercase alphanumeric/hyphen: {name}")
    return (owner, f"prometheus-{stream}" if stream.startswith("prometheus-") else stream)


def build_snapshot_widget(payload: dict, widget_id: str) -> dict:
    """
    Transform an SSE event payload into a widget snapshot.

    The event payload is expected to be a JSON object that matches one of
    the Canvas widget kinds (stat, gauge, time_series, etc.). It is passed
    through directly as the widget spec — the binder in Canvas handles
    further transformation if needed.

    If the payload is not a full widget spec, it is wrapped in a table
    or event_log so something renders.
    """
    if "kind" in payload and payload["kind"] in [
        "stat", "gauge", "meter", "time_series", "heatmap",
        "ranked_bar", "stacked_bar", "table", "event_log",
        "node_summary", "clock", "ticker", "status", "text",
        "slo_burn_rate", "state_timeline", "chart",
    ]:
        # Already a widget spec — wrap it with identity.
        widget = dict(payload)
        widget.setdefault("schema_version", 1)
        widget.setdefault("id", widget_id)
        return widget

    # Fallback: if the payload has "entries" treat as event_log
    if "entries" in payload:
        return {
            "schema_version": 1,
            "id": widget_id,
            "kind": "event_log",
            **payload,
        }

    # Fallback: if it has "rows" and "columns" treat as table
    if "rows" in payload and "columns" in payload:
        return {
            "schema_version": 1,
            "id": widget_id,
            "kind": "table",
            **payload,
        }

    # Fallback: if it has a "value" treat as stat
    if "value" in payload:
        return {
            "schema_version": 1,
            "id": widget_id,
            "kind": "stat",
            "label": payload.get("label", widget_id),
            "value": float(payload["value"]),
            **{k: v for k, v in payload.items()
              if k not in ("value", "label", "kind", "schema_version", "id")},
        }

    # Last resort: text
    return {
        "schema_version": 1,
        "id": widget_id,
        "kind": "text",
        "text": json.dumps(payload)[:4096],
    }


def read_sse_events(url: str):
    """
    Generator yielding SSE event data as parsed JSON dicts.

    Connects to the URL and reads the event stream. Each SSE event with
    a 'data:' field is parsed as JSON. Events without data or with invalid
    JSON are logged to stderr and skipped.
    """
    req = urllib.request.Request(url, headers={
        "Accept": "text/event-stream",
    })
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            if response.status != 200:
                raise RuntimeError(f"SSE endpoint returned HTTP {response.status}")
            # Read line by line
            buffer = ""
            for line in response:
                try:
                    line_str = line.decode("utf-8").rstrip("\r\n")
                except UnicodeDecodeError:
                    continue
                if line_str == "":
                    # Empty line = event boundary
                    if buffer.strip():
                        event_data = parse_sse_data(buffer)
                        if event_data is not None:
                            yield event_data
                    buffer = ""
                else:
                    buffer += line_str + "\n"
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"SSE connection failed: {e.code} {e.reason}")


def parse_sse_data(raw: str) -> dict | None:
    """Parse SSE 'data:' field as JSON."""
    for line in raw.split("\n"):
        if line.startswith("data:"):
            data = line[5:].strip()
            if data:
                try:
                    return json.loads(data)
                except json.JSONDecodeError:
                    return {"raw": data}
    return None


def main():
    parser = argparse.ArgumentParser(
        description="SSE-to-Canvas stream connector"
    )
    parser.add_argument("--socket", required=True,
                        help="Path to Canvas stream.sock")
    parser.add_argument("--url", required=True,
                        help="SSE endpoint URL")
    parser.add_argument("--owner", required=True,
                        help="Stream owner (e.g. 'elmers')")
    parser.add_argument("--stream", required=True,
                        help="Stream id (e.g. 'activity-events')")
    parser.add_argument("--widget-id", default="from-sse",
                        help="Default widget id for snapshots")
    parser.add_argument("--reconnect-delay", type=float, default=5.0,
                        help="Seconds to wait before reconnecting")
    args = parser.parse_args()

    print(f"SSE connector: {args.url} -> {args.owner}/{args.stream}", flush=True)

    seq = 0
    while True:
        try:
            for event in read_sse_events(args.url):
                seq += 1
                widget = build_snapshot_widget(event, args.widget_id)
                envelope = {
                    "schema_version": STREAM_SCHEMA_VERSION,
                    "owner": args.owner,
                    "stream": args.stream,
                    "contract": CONTRACT,
                    "sequence": seq,
                    "emitted_at_ms": now_ms(),
                    "payload": {
                        "widget": widget,
                        "data": {
                            "state": "ready",
                            "source": f"sse:{args.owner}/{args.stream}",
                            "provenance": "live",
                        },
                    },
                }
                if send_event(args.socket, envelope):
                    print(f"Sent snapshot #{seq} ({widget['kind']})", flush=True)
                else:
                    print(f"Send failed for snapshot #{seq}", file=sys.stderr, flush=True)
        except (RuntimeError, ConnectionError, OSError) as e:
            print(f"SSE connection error: {e}", file=sys.stderr, flush=True)
            print(f"Reconnecting in {args.reconnect_delay}s...", flush=True)
            time.sleep(args.reconnect_delay)
        except KeyboardInterrupt:
            print("\nSSE connector stopped.", flush=True)
            sys.exit(0)
