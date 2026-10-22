#!/usr/bin/env python3
"""General metrics feed for Canvas.

Collects useful operational data — MPD now-playing, GitHub activity across
all orgs (recent commits, open PRs, new stars) — and sends as table/widget
snapshots to Canvas's stream.sock.

Usage:
  python3 canvas-metrics-feed.py --every 30

Environment:
  MPD_HOST   — MPD server host (default: 127.0.0.1)
  MPD_PORT   — MPD port (default: 6600)
  GITHUB_TOKEN — GitHub API token (or uses `gh` CLI auth)
  FPL_GITHUB_ORGS — Comma-separated orgs to monitor (default: "FuturePresentLabs")
"""
import json
import os
import socket
import subprocess
import sys
import time
import urllib.request
import urllib.parse

CONTRACT = "canvas.widget.snapshot.v1"
SOCKET_PATH = os.path.join(
    os.environ.get("HOME", ""), "Library/Application Support/Dock/canvas/stream.sock"
)
MPD_HOST = os.environ.get("MPD_HOST", "127.0.0.1")
MPD_PORT = int(os.environ.get("MPD_PORT", "6600"))
GITHUB_ORGS = os.environ.get("FPL_GITHUB_ORGS", "FuturePresentLabs").split(",")
GITHUB_TOKEN = os.environ.get("GITHUB_TOKEN", "")

def send_widget(owner, stream, title, columns, rows, kind="table"):
    """Send a widget snapshot to Canvas."""
    if not os.path.exists(SOCKLE_PATH := SOCKET_PATH):
        return False
    event = {
        "schema_version": 1,
        "owner": owner,
        "stream": stream,
        "contract": CONTRACT,
        "sequence": int(time.time() * 1000),
        "emitted_at_ms": int(time.time() * 1000),
        "payload": {
            "total": len(rows),
            "running": 0,
            "queued": 0,
            "limit": 10,
            "jobs": [
                {"state": "active", "repo": row[0], "command": row[1] if len(row) > 1 else "", "waited": 0, "ran": 0}
                for row in rows[:10]
            ],
            "title": title,
            "columns": columns,
        },
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.sendto(json.dumps(event, separators=(",", ":")).encode(), SOCKET_PATH)
        sock.close()
        return True
    except Exception:
        return False

def mpd_now_playing():
    """Get current MPD status via mpc or raw protocol."""
    # Try mpc first
    try:
        result = subprocess.run(
            ["mpc", "--host", MPD_HOST, "--port", str(MPD_PORT), "current", "-f", "%artist% - %title%"],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    except Exception:
        pass
    # Fallback: raw MPD protocol
    try:
        sock = socket.create_connection((MPD_HOST, MPD_PORT), timeout=5)
        sock.sendall(b"id\n")
        sock.recv(4096)  # consume greeting
        sock.sendall(b"currentsong\n")
        response = b""
        while True:
            data = sock.recv(4096)
            if not data:
                break
            response += data
            if b"OK\n" in response:
                break
        sock.close()
        # Parse response
        lines = response.decode("utf-8", errors="replace").strip().split("\n")
        artist = title = ""
        for line in lines:
            if line.startswith("artist "):
                artist = line[7:]
            elif line.startswith("title "):
                title = line[6:]
        if artist or title:
            return f"{artist} - {title}".strip(" -")
    except Exception:
        pass
    return "MPD: stopped"

def github_api(path):
    """Call GitHub API using gh CLI auth or token."""
    headers = {"Accept": "application/vnd.github+json"}
    if GITHUB_TOKEN:
        headers["Authorization"] = f"token {GITHUB_TOKEN}"
    url = f"https://api.github.com{path}"
    try:
        req = urllib.request.Request(url, headers=headers)
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read().decode())
    except Exception:
        # Try gh CLI as fallback
        result = subprocess.run(
            ["gh", "api", path, "-H", "Accept: application/vnd.github+json"],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode == 0:
            return json.loads(result.stdout)
    return None

def github_stats():
    """Collect GitHub stats across all configured orgs."""
    rows = []
    for org in GITHUB_ORGS:
        # Open PRs
        prs = github_api(f"/repos/{org}/dock/pulls?state=open&per_page=5") or []
        for pr in prs:
            rows.append((f"PR #{pr.get('number','?')}", f"{pr.get('title','?')[:40]}"))

        # Recent commits
        commits = github_api(f"/repos/{org}/dock/commits?per_page=3") or []
        for commit in commits:
            msg = commit.get("commit", {}).get("message", "")[:40]
            rows.append(("Recent commit", msg))

        # Stars (org-level)
        org_data = github_api(f"/orgs/{org}")
        if org_data:
            public = org_data.get("public_repos", 0)
            rows.append(("Org repos", f"{org}: {public} public repos"))

    # Recent stars across org repos
    for org in GITHUB_ORGS:
        starred = github_api(f"/users/{GITHUB_ORGS[0]}/starred?per_page=5") or []
        for repo in starred[:3]:
            full = repo.get("full_name", "?")
            rows.append(("New star", full))

    return rows

def main():
    import argparse
    parser = argparse.ArgumentParser(description="General metrics feed for Canvas")
    parser.add_argument("--every", type=float, default=30, help="Poll interval in seconds")
    args = parser.parse_args()

    if not os.path.exists(SOCKLE_PATH := SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        return

    print(f"canvas-metrics-feed: MPD={MPD_HOST}:{MPD_PORT} orgs={GITHUB_ORGS}", file=sys.stderr)

    while True:
        timestamp = time.strftime("%H:%M")

        # Send MPD now-playing
        playing = mpd_now_playing()
        send_widget(
            "metrics", "mpd-now-playing",
            f"Now playing ({timestamp})",
            ["Track"],
            [(playing,)],
            "table"
        )

        # Send GitHub stats
        gh_rows = github_stats()
        if gh_rows:
            send_widget(
                "metrics", "github-activity",
                "GitHub activity",
                ["Type", "Detail"],
                gh_rows,
                "table"
            )
        else:
            send_widget(
                "metrics", "github-activity",
                "GitHub activity",
                ["Type", "Detail"],
                [("No activity", "GitHub API unavailable")],
                "table"
            )

        print(f"  metrics sent: mpd={playing[:30]} gh={len(gh_rows)} rows", file=sys.stderr)
        time.sleep(args.every)

if __name__ == "__main__":
    main()
