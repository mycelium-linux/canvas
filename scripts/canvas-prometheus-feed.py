#!/usr/bin/env python3
"""Forward Prometheus metrics to Canvas as widget snapshots.

Follows the same local-relay pattern as canvas-sse-feed.py and
canvas-repo-feed.py: this process holds Prometheus credentials/endpoints
and writes canvas.widget.snapshot.v1 datagrams to stream.sock.

  canvas-prometheus-feed.py \
    --prometheus http://127.0.0.1:9090 \
    --owner bifrost \
    --metric health \
    --kind stat \
    --label "Backend health" \
    --query 'up{job="bifrost"}' \
    --interval 10

Widget kinds: stat, gauge, time_series, ranked_bar, heatmap, state_timeline,
slo_burn_rate, sparkline, histogram, event_log.

The --window flag controls whether a range query is used ('5m', '1h', '24h')
or an instant query ('now').
"""
import argparse
import json
import os
import socket
import sys
import time
import urllib.parse
import urllib.request
import urllib.error

CONTRACT = "canvas.widget.snapshot.v1"
SOCKET_PATH = os.path.join(
    os.environ.get("HOME", ""), "Library/Application Support/Dock/canvas/stream.sock"
)

# Color constants matching Coherence palette for status states
COLORS = {
    "good": "#10b981",
    "warning": "#f59e0b",
    "danger": "#ef4444",
    "neutral": "#9ca3af",
    "stale": "#6b7280",
}


def send_envelope(owner, stream, payload, sequence):
    event = {
        "schema_version": 1,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": sequence,
        "emitted_at_ms": int(time.time() * 1000),
        "payload": payload,
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.sendto(json.dumps(event, separators=(",", ":")).encode(), SOCKET_PATH)
        sock.close()
    except Exception as exc:
        print(f"canvas-prometheus-feed: send failed: {exc}", file=sys.stderr)


def prom_query(prom_url, query, window=None):
    """Query Prometheus and return a list of (timestamp_ms, value) pairs."""
    if window:
        # Range query
        params = urllib.parse.urlencode({
            "query": query,
            "start": int(time.time()) - int(window[:-1]) * 60 if window.endswith("m") else int(time.time()),
            "end": int(time.time()),
            "step": window,
        })
    else:
        # Instant query
        params = urllib.parse.urlencode({"query": query})

    url = f"{prom_url}/api/v1/query{'Range' if window else ''}?" + params
    try:
        req = urllib.request.Request(url, headers={"Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.loads(resp.read().decode())
            if data.get("status") != "success":
                return None
            result = data.get("data", {}).get("result", [])
            points = []
            for series in result:
                values = series.get("values", [])
                for ts, val in values:
                    try:
                        v = float(val)
                    except (ValueError, TypeError):
                        continue
                    points.append((int(float(ts) * 1000), v))
            points.sort()
            return points
    except urllib.error.URLError as exc:
        print(f"canvas-prometheus-feed: Prometheus query failed: {exc}", file=sys.stderr)
        return None
    except json.JSONDecodeError as exc:
        print(f"canvas-prometheus-feed: Prometheus response decode failed: {exc}", file=sys.stderr)
        return None


def build_widget(kind, label, points, unit, colors):
    """Build a widget snapshot payload for a given kind."""
    now_value = points[-1][1] if points else 0.0
    first_value = points[0][1] if points else 0.0

    if not points:
        data = {"state": "no_data", "source": "prometheus"}
    elif now_value == first_value:
        data = {"state": "ready", "source": "prometheus"}
    else:
        data = {"state": "ready", "source": "prometheus"}

    if kind == "stat":
        return {
            "widget": {
                "kind": "stat",
                "label": label,
                "value": round(now_value, 2),
                "unit": unit,
                "state": "good" if now_value >= 0.5 else "danger",
            },
            "data": data,
        }
    elif kind == "gauge":
        max_val = max(p[1] for p in points) if points else 100.0
        pct = (now_value / max(max_val, 1.0)) * 100.0 if max_val else 0.0
        return {
            "widget": {
                "kind": "gauge",
                "value": round(pct, 1),
                "label": label,
                "unit": unit,
            },
            "data": data,
        }
    elif kind == "time_series":
        series = [{"label": label, "points": [[ts, v] for ts, v in points[-60:]]}]
        return {
            "widget": {
                "kind": "time_series",
                "title": label,
                "series": series,
                "unit": unit,
            },
            "data": data,
        }
    elif kind == "ranked_bar":
        bar = [(label, now_value, None)]
        return {
            "widget": {
                "kind": "ranked_bar",
                "title": label,
                "unit": unit,
                "bars": [{"label": l, "value": v, "state": s} for l, v, s in bar],
            },
            "data": data,
        }
    elif kind == "heatmap":
        # Single metric as a 1x1 heatmap
        return {
            "widget": {
                "kind": "heatmap",
                "title": label,
                "rows": [label],
                "columns": [label],
                "values": [[round(now_value, 2)]],
                "unit": unit,
            },
            "data": data,
        }
    elif kind == "sparkline":
        return {
            "widget": {
                "kind": "sparkline",
                "values": [p[1] for p in points[-20:]],
                "color": COLORS.get("good", "#10b981"),
            },
            "data": data,
        }
    else:
        # Fallback: emit as a stat
        return {
            "widget": {
                "kind": "stat",
                "label": label,
                "value": round(now_value, 2),
                "unit": unit,
                "state": "good" if now_value > 0 else "danger",
            },
            "data": data,
        }


def main():
    parser = argparse.ArgumentParser(
        description="Forward Prometheus metrics to Canvas as widget snapshots"
    )
    parser.add_argument("--prometheus", required=True, help="Prometheus HTTP URL")
    parser.add_argument("--owner", required=True, help="Stream owner (project id)")
    parser.add_argument("--metric", required=True, help="Metric name (becomes stream name)")
    parser.add_argument("--kind", default="stat", help="Widget kind")
    parser.add_argument("--label", default="", help="Widget label/title")
    parser.add_argument("--query", required=True, help="PromQL query")
    parser.add_argument("--window", default=None,
                        help="Range query window (e.g. '5m', '1h', '24h'). If omitted, instant query.")
    parser.add_argument("--unit", default="", help="Unit label")
    parser.add_argument("--interval", type=float, default=10, help="Poll interval in seconds")
    args = parser.parse_args()

    if not os.path.exists(SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        return

    stream = f"prometheus-{args.metric}"
    label = args.label or args.metric
    window_seconds = None
    if args.window:
        w = args.window
        if w.endswith("m"):
            window_seconds = int(w[:-1]) * 60
        elif w.endswith("h"):
            window_seconds = int(w[:-1]) * 3600
        elif w.endswith("s"):
            window_seconds = int(w[:-1])
        else:
            window_seconds = int(w)

    print(f"canvas-prometheus-feed: {args.prometheus} -> {args.owner}/{stream} ({args.kind})",
          file=sys.stderr)
    print(f"  query: {args.query}  window: {args.window or 'instant'}  interval: {args.interval}s",
          file=sys.stderr)

    sequence = 0
    errors = 0
    while True:
        points = prom_query(args.prometheus, args.query, args.window)
        if points is None:
            errors += 1
            # Emit a stale state on error
            payload = {
                "widget": {
                    "kind": args.kind or "stat",
                    "label": label,
                    "value": 0,
                    "state": "danger" if errors == 1 else "stale",
                },
                "data": {
                    "state": "error",
                    "source": "prometheus",
                    "error": "query failed",
                    "consecutive_errors": errors,
                },
            }
            sequence += 1
            send_envelope(args.owner, stream, payload, sequence)
        else:
            errors = 0
            payload = build_widget(args.kind, label, points, args.unit, COLORS)
            sequence += 1
            send_envelope(args.owner, stream, payload, sequence)

        time.sleep(args.interval)


if __name__ == "__main__":
    main()
