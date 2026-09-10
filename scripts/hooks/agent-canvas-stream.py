#!/usr/bin/env python3
"""Sample OpenCode activity plugin: stream agent actions to Canvas.

Pipe OpenCode transcript events to this script, or have OpenCode invoke it
as a hook after each tool call:

  opencode --plugin hooks/agent-canvas-stream.py

The script sends one UDP datagram per activity event to Canvas's stream
socket with contract `canvas.agent.activity.v1`. The Canvas binder
(`config/binders/agent-activity-v1.lua`) transforms it into a live table
surface showing what the agent is touching.

Event format (JSON on stdin):
  {"tool":"str_replace","action":"edit","file":"path/to/file","project":"dock","success":true}
"""
import json, os, socket, sys, time

CONTRACT = "canvas.agent.activity.v1"
SCHEMA_VERSION = 1

def main():
    home = os.environ.get("HOME", "")
    socket_path = os.path.join(
        home, "Library/Application Support/Dock/canvas/stream.sock"
    )
    if not os.path.exists(socket_path):
        sys.exit(f"no Canvas stream socket at {socket_path} — start Canvas first")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sequence = 0

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue

        sequence += 1
        payload = {
            "tool": event.get("tool", ""),
            "action": event.get("action", "tool"),
            "file": event.get("file", ""),
            "project": event.get("project", ""),
            "duration_ms": event.get("duration_ms"),
            "success": event.get("success", True),
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
            # The binder also receives the current window of existing rows,
            # so it can append instead of replacing. For now, just the new event.
        }
        envelope = {
            "schema_version": SCHEMA_VERSION,
            "owner": event.get("owner", "agent"),
            "stream": event.get("stream", "agent-activity"),
            "contract": CONTRACT,
            "sequence": sequence,
            "emitted_at_ms": int(time.time() * 1000),
            "payload": payload,
        }
        body = json.dumps(envelope, separators=(",", ":")).encode()
        if len(body) > 64 * 1024:
            continue
        try:
            sock.sendto(body, socket_path)
        except OSError as error:
            print(f"agent-canvas-stream: send failed: {error}", file=sys.stderr)


if __name__ == "__main__":
    main()
