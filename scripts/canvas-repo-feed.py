#!/usr/bin/env python3
"""Stream Alfalfa repo/PR status to Canvas as a table widget.

Polls Alfalfa's /api/v1/repos endpoint and sends per-repo status as a
lockout-formatted table. Shows active sessions, ready/backlog counts,
and idle ratio (utilization) per repo.
"""
import json, os, socket, time, urllib.request

SOCKET_PATH = os.path.join(
    os.environ.get("HOME", ""), "Library/Application Support/Dock/canvas/stream.sock"
)
ALFALFA_URL = os.environ.get("FPL_ALFALFA_URL", "http://127.0.0.1:8787")
CONTRACT = "canvas.widget.snapshot.v1"

def fetch_repos():
    try:
        with urllib.request.urlopen(f"{ALFALFA_URL}/api/v1/repos", timeout=5) as resp:
            return json.loads(resp.read().decode())
    except: return []

def send_table(repos):
    jobs = []
    for repo in sorted(repos, key=lambda r: r.get("id", "")):
        jobs.append({
            "id": repo.get("id", "?"),
            "state": "active" if repo.get("active", 0) > 0 else "idle",
            "waited": repo.get("backlog", 0),
            "ran": repo.get("ready", 0),
            "cwd": repo.get("workspace", "?"),
            "command": repo.get("id", "")[:30],
            "note": f"idle:{repo.get('idle_ratio',0):.0%} cap:{repo.get('capacity','?')}",
        })
    
    event = {
        "schema_version": 1,
        "owner": "alfalfa",
        "stream": "repo-status",
        "contract": CONTRACT,
        "sequence": int(time.time() * 1000000) % 1000000,
        "emitted_at_ms": int(time.time() * 1000),
        "payload": {
            "limit": 100,
            "running": sum(1 for r in repos if r.get("active", 0) > 0),
            "queued": sum(r.get("backlog", 0) for r in repos),
            "total": len(repos),
            "jobs": jobs[:12],
        }
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.sendto(json.dumps(event, separators=(",", ":")).encode(), SOCKET_PATH)
        sock.close()
    except: pass

def main():
    if not os.path.exists(SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=__import__('sys').stderr)
        return
    print(f"canvas-repo-feed: polling {ALFALFA_URL}/api/v1/repos", file=__import__('sys').stderr)
    while True:
        repos = fetch_repos()
        if repos:
            send_table(repos)
        time.sleep(10)

if __name__ == "__main__":
    main()
