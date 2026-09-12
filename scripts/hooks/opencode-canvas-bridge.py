#!/usr/bin/env python3
"""Forward OpenCode events to Canvas.

Watches OpenCode's log output and extracts tool calls, file edits, and
agent state changes. Each event is sent as a UDP datagram to Canvas's
stream socket with contract canvas.agent.activity.v1.

The Canvas binder (agent-activity-v1.lua) transforms these into a live
widget showing what the agent is touching — the ghost in the shell.

Usage:
  python3 opencode-canvas-bridge.py --log /path/to/opencode.log
  # Or pipe OpenCode stdout:
  opencode | python3 opencode-canvas-bridge.py --stdin
"""
import argparse, json, os, socket, sys, time, re

CONTRACT = "canvas.agent.activity.v1"
home = os.environ.get("HOME", "")
SOCKET_PATH = os.path.join(home, "Library/Application Support/Dock/canvas/stream.sock")

def send_event(event_type, tool, file=None, project="dock", success=True, duration=None):
    envelope = {
        "schema_version": 1,
        "owner": "agent",
        "stream": "agent-activity",
        "contract": CONTRACT,
        "sequence": int(time.time() * 1000),
        "emitted_at_ms": int(time.time() * 1000),
        "payload": {
            "tool": tool,
            "action": event_type,
            "file": file or "",
            "project": project,
            "duration_ms": duration or 0,
            "success": success,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        }
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        body = json.dumps(envelope, separators=(",", ":")).encode()
        sock.sendto(body, SOCKET_PATH)
        sock.close()
        print(f"  → {event_type} {tool} {file or ''}", file=sys.stderr)
    except Exception:
        pass

def main():
    ap = argparse.ArgumentParser(description="Forward OpenCode events to Canvas")
    ap.add_argument("--stdin", action="store_true", help="Read OpenCode output from stdin")
    ap.add_argument("--log", help="Tail a log file")
    args = ap.parse_args()

    if not os.path.exists(SOCKET_PATH):
        print(f"No Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        sys.exit(1)

    print(f"Forwarding to {SOCKET_PATH}", file=sys.stderr)

    if args.stdin:
        for line in sys.stdin:
            parse_opencode_line(line.strip())
    elif args.log:
        with open(args.log) as f:
            for line in f:
                parse_opencode_line(line.strip())
    else:
        # Demo mode: inject sample events
        print("No source specified. Demo mode: sending sample events.", file=sys.stderr)
        demo_events()

def parse_opencode_line(line):
    """Parse OpenCode output for tool calls and file edits."""
    if not line:
        return
    # Look for common OpenCode patterns
    if "str_replace" in line or "str_replace_editor" in line:
        m = re.search(r'file[=: "]*([^\s"\'\]]+)', line)
        file = m.group(1) if m else None
        action = "edit" if "str_replace" in line else "write"
        send_event(action, "str_replace", file)
    elif "read_file" in line or "read" in line and "file" in line:
        send_event("read", "read_file")
    elif "bash" in line.lower() or "run" in line.lower():
        send_event("tool", "bash")
    elif "thinking" in line.lower() or "thinking" in line:
        send_event("state", "thinking")

def demo_events():
    events = [
        ("read", "read_file", "crates/canvas/src/main.rs"),
        ("edit", "str_replace", "crates/canvas/src/main.rs"),
        ("tool", "bash", "cargo build -p canvas"),
        ("read", "read_file", "crates/canvas/src/mcp.rs"),
        ("edit", "str_replace", "crates/canvas/src/mcp.rs"),
        ("tool", "bash", "cargo test --workspace"),
    ]
    for action, tool, file in events:
        send_event(action, tool, file)
        time.sleep(0.5)

if __name__ == "__main__":
    main()
