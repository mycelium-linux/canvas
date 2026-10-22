#!/usr/bin/env python3
"""Forward OpenCode agent attention events to Canvas.

When an agent reads, edits, writes, or searches, it emits a
canvas.agent.attention.v1 envelope to stream.sock. The agent-attention-v1
Lua binder transforms these into a table surface showing the latest
attention signal.

The agent should call this script after each notable action:

  python3 canvas-agent-attention.py --kind edit --target crates/canvas/src/main.rs --region "line:42"

Or pipe JSON events from stdin:

  opencode | python3 canvas-agent-attention.py --stdin --project canvas
"""
import argparse
import json
import os
import socket
import sys
import time

CONTRACT = "canvas.agent.attention.v1"
SOCKET_PATH = os.path.join(
    os.environ.get("HOME", ""), "Library/Application Support/Dock/canvas/stream.sock"
)

KIND_LABELS = {
    "read": "Read",
    "edit": "Edit",
    "write": "Write",
    "search": "Search",
}


def send_attention(project, kind, target, region=None, at=None, sequence=None):
    if not os.path.exists(SOCKLE_PATH := SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        return False
    if kind not in KIND_LABELS:
        print(f"unknown attention kind: {kind}. Use one of: {', '.join(KIND_LABELS)}", file=sys.stderr)
        return False
    if not project:
        print("missing required --project", file=sys.stderr)
        return False
    if not target:
        print("missing required --target", file=sys.stderr)
        return False
    if sequence is None:
        sequence = int(time.time() * 1000) % 1000000
    if at is None:
        at = time.strftime("%Y-%m-%dT%H:%M:%S")
    event = {
        "schema_version": 1,
        "owner": project,
        "stream": "agent-attention",
        "contract": CONTRACT,
        "project_id": project,
        "sequence": sequence,
        "emitted_at_ms": int(time.time() * 1000),
        "payload": {
            "project": project,
            "kind": kind,
            "target": target,
            "region": region or "",
            "at": at,
        },
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.sendto(json.dumps(event, separators=(",", ":")).encode(), SOCKET_PATH)
        sock.close()
        return True
    except Exception as exc:
        print(f"canvas-agent-attention: send failed: {exc}", file=sys.stderr)
        return False


def main():
    parser = argparse.ArgumentParser(
        description="Forward agent attention events to Canvas"
    )
    parser.add_argument("--project", default="dock", help="Project the agent is working in")
    parser.add_argument("--kind", choices=list(KIND_LABELS.keys()),
                        help="Attention kind: read, edit, write, search")
    parser.add_argument("--target", help="File path or search query")
    parser.add_argument("--region", default="", help="Optional region (e.g. 'line:42')")
    parser.add_argument("--stdin", action="store_true",
                        help="Read JSON events from stdin")
    args = parser.parse_args()

    if args.stdin:
        sequence = 0
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError as exc:
                print(f"canvas-agent-attention: skip invalid JSON: {exc}", file=sys.stderr)
                continue
            sequence += 1
            send_attention(
                project=data.get("project", args.project),
                kind=data.get("kind", args.kind or "read"),
                target=data.get("target", args.target or ""),
                region=data.get("region", args.region),
                at=data.get("at"),
                sequence=sequence,
            )
    else:
        if not send_attention(args.project, args.kind, args.target, args.region):
            sys.exit(1)


if __name__ == "__main__":
    main()
