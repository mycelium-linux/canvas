#!/usr/bin/env python3
"""Feed `lockout status --json` to Canvas as a widget snapshot.

No compile step and no Rust: the producer only bounds the payload to what one
datagram holds, and `config/binders/lockout-v1.lua` does every bit of the
formatting. Changing how the queue looks is editing the Lua file.
"""
import json
import os
import socket
import subprocess
import sys
import time

CONTRACT = "canvas.widget.snapshot.v1"
SCHEMA_VERSION = 1
# macOS caps an AF_UNIX datagram at net.local.dgram.maxdgram, 2048 by default,
# and an oversized send fails outright rather than truncating.
MAX_DATAGRAM = 2048
# Generous: the binder trims for display, this only stops one pathological
# command line from eating the whole budget.
MAX_COMMAND = 160


def arg(flag, fallback=None):
    argv = sys.argv
    if flag in argv:
        index = argv.index(flag)
        if index + 1 < len(argv):
            return argv[index + 1]
    return fallback


def payload(status, keep):
    jobs = status.get("jobs", [])
    return {
        "limit": status.get("limit", 0),
        "running": status.get("running", 0),
        "queued": status.get("queued", 0),
        # Sent explicitly so the binder can report the real queue length even
        # though the wire only carries `keep` of them.
        "total": len(jobs),
        "jobs": [
            {
                "id": job.get("id", ""),
                "state": job.get("state", ""),
                "waited": job.get("waited", -1),
                "ran": job.get("ran", -1),
                "cwd": job.get("cwd", ""),
                "command": (job.get("command", "") or "")[:MAX_COMMAND],
                "note": job.get("note", "") or "",
            }
            for job in jobs[:keep]
        ],
    }


def main():
    home = os.environ.get("HOME", "")
    default_socket = os.path.join(
        home, "Library/Application Support/Dock/canvas/stream.sock"
    )
    path = arg("--socket", default_socket)
    owner = arg("--owner", "machine")
    stream = arg("--stream", "lockout")
    every = float(arg("--every", "2"))
    if not os.path.exists(path):
        sys.exit(f"no Canvas stream socket at {path} - start Canvas first")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sequence = 0
    print(f"canvas-lockout-feed: {owner}/{stream} every {every}s -> {path}", file=sys.stderr)
    while True:
        try:
            raw = subprocess.run(
                ["lockout", "status", "--json"],
                capture_output=True, text=True, timeout=10,
            ).stdout
            status = json.loads(raw)
        except Exception as error:  # fail loud: a stale table is a lie
            print(f"canvas-lockout-feed: cannot read lockout: {error}", file=sys.stderr)
            time.sleep(every)
            continue

        sent = False
        for keep in (12, 10, 8, 6, 4, 2, 1):
            sequence += 1
            event = {
                "schema_version": SCHEMA_VERSION,
                "owner": owner,
                "stream": stream,
                "contract": CONTRACT,
                "sequence": sequence,
                "emitted_at_ms": int(time.time() * 1000),
                "payload": payload(status, keep),
            }
            body = json.dumps(event, separators=(",", ":")).encode()
            if len(body) > MAX_DATAGRAM:
                continue
            try:
                sock.sendto(body, path)
                sent = True
            except OSError as error:
                print(f"canvas-lockout-feed: send failed: {error}", file=sys.stderr)
                sent = True  # reported; do not retry smaller for a socket fault
            break
        if not sent:
            print("canvas-lockout-feed: one job did not fit a datagram", file=sys.stderr)
        time.sleep(every)


if __name__ == "__main__":
    main()
