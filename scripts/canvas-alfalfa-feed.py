#!/usr/bin/env python3
"""Alfalfa activity feed: recent Beads, PRs, and active projects.

Polls the Beads tracker and GitHub for the Alfalfa project, formats the
results as a table widget, and sends canvas.widget.snapshot.v1 datagrams
to Canvas's stream.sock.

This replaces the lockout repo-status table on the Alfalfa workspace with
actual project activity: what Beads just closed, what PRs are open, what
projects are active.

Usage:
  python3 canvas-alfalfa-feed.py --every 10

Configures via environment:
  FPL_ALFALFA_DIR  — Alfalfa repo path (default: ~/src/fpl/alfalfa)
  FPL_GITHUB_REPO  — GitHub repo (default: derived from git remote)
"""
import json
import os
import socket
import subprocess
import sys
import time
import urllib.request

CONTRACT = "canvas.widget.snapshot.v1"
HOME = os.environ.get("HOME", "")
SOCKET_PATH = os.path.join(HOME, "Library/Application Support/Dock/canvas/stream.sock")
ALFALFA_DIR = os.environ.get("FPL_ALFALFA_DIR", os.path.expanduser("~/src/fpl/alfalfa"))
ALFALFA_URL = os.environ.get("FPL_ALFALFA_URL", "http://127.0.0.1:8787")
GH_REPO = os.environ.get("FPL_GITHUB_REPO", "")

MAX_ROWS = 12


def run_cmd(cmd, cwd=None, timeout=5):
    """Run a command and return stdout, or empty string on failure."""
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True,
            timeout=timeout, cwd=cwd,
        )
        return result.stdout.strip() if result.returncode == 0 else ""
    except Exception:
        return ""


def get_github_repo():
    """Get the GitHub repo from git remote."""
    if GH_REPO:
        return GH_REPO
    remote = run_cmd(["git", "remote", "get-url", "origin"], cwd=ALFALFA_DIR, timeout=3)
    if "github.com" in remote:
        # Extract owner/repo from git@github.com:owner/repo.git or https://github.com/owner/repo
        parts = remote.replace("git@github.com:", "").replace("https://github.com/", "").replace(".git", "").strip()
        return parts
    return "FuturePresentLabs/alfalfa"


def fetch_recent_beads(limit=8):
    """Get recently closed/open Beads from bd."""
    output = run_cmd(["bd", "list", "--all"], cwd=ALFALFA_DIR, timeout=10)
    beads = []
    for line in output.split("\n"):
        if line.startswith("○") or line.startswith("✓") or line.startswith("◐"):
            # Parse: ○ dock-cov.37 ● P0 [bug] ...  or  ✓ dock-cov.35 ● ...
            parts = line.split()
            if len(parts) < 3:
                continue
            bead_id = parts[1] if parts[1].startswith("dock") else "?"
            # Extract status marker
            status = "open" if "○" in line[0] else "closed" if "✓" in line else "in_progress"
            # Extract title (everything after the priority/type tags)
            title = " ".join(parts[4:]) if len(parts) > 4 else bead_id
            # Truncate title
            if len(title) > 50:
                title = title[:47] + "..."
            beads.append((bead_id, title, status))
            if len(beads) >= limit:
                break
    return beads


def fetch_recent_prs(limit=8):
    """Get recent PRs from GitHub."""
    repo = get_github_repo()
    output = run_cmd(["gh", "pr", "list", "--repo", repo, "--state", "open", "--limit", str(limit), "--json", "number,title,state"], timeout=10)
    if not output:
        return []
    try:
        prs = json.loads(output)
        return [(f"#{pr['number']}", pr['title'][:47], pr['state']) for pr in prs]
    except (json.JSONDecodeError, KeyError):
        return []


def fetch_active_projects():
    """Get active projects from Alfalfa API."""
    try:
        with urllib.request.urlopen(f"{ALFALFA_URL}/api/v1/projects", timeout=5) as resp:
            data = json.loads(resp.read().decode())
            if isinstance(data, list):
                return [(p.get("project", {}).get("id", "?"), p.get("project", {}).get("name", "?"), "active") for p in data[:5]]
    except Exception:
        pass
    # Fallback: derive from repos
    try:
        with urllib.request.urlopen(f"{ALFALFA_URL}/api/v1/repos", timeout=5) as resp:
            data = json.loads(resp.read().decode())
            if isinstance(data, list):
                return [(r.get("id", "?"), r.get("id", "?"), r.get("state", "?")) for r in data[:5]]
    except Exception:
        pass
    return []


def send_table(items):
    """Send the activity table to Canvas."""
    if not os.path.exists(SOCKLE_PATH := SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        return False
    rows = []
    for item_id, title, state in items:
        rows.append({
            "state": "active" if state in ("active", "OPEN", "open") else "idle",
            "repo": item_id,
            "command": title[:30],
            "waited": 0,
            "ran": 0,
            "note": f"state={state}",
        })
    if not rows:
        rows = [{"state": "idle", "repo": "none", "command": "No activity", "waited": 0, "ran": 0, "note": "idle"}]

    event = {
        "schema_version": 1,
        "owner": "alfalfa",
        "stream": "repo-status",
        "contract": CONTRACT,
        "sequence": int(time.time() * 1000),
        "emitted_at_ms": int(time.time() * 1000),
        "payload": {
            "total": len(items),
            "running": sum(1 for _, _, s in items if s in ("active", "OPEN", "open")),
            "queued": 0,
            "limit": MAX_ROWS,
            "jobs": rows[:MAX_ROWS],
        },
    }
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        sock.sendto(json.dumps(event, separators=(",", ":")).encode(), SOCKET_PATH)
        sock.close()
        return True
    except Exception as exc:
        print(f"canvas-alfalfa-feed: send failed: {exc}", file=sys.stderr)
        return False


def main():
    import argparse
    parser = argparse.ArgumentParser(description="Alfalfa activity feed for Canvas")
    parser.add_argument("--every", type=float, default=15, help="Poll interval in seconds")
    args = parser.parse_args()

    if not os.path.exists(SOCKLE_PATH := SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=sys.stderr)
        return

    print(f"canvas-alfalfa-feed: polling Beads, GitHub PRs, and Alfalfa API", file=sys.stderr)
    print(f"  repo: {get_github_repo()}  alfalfa: {ALFALFA_URL}", file=sys.stderr)

    while True:
        # Collect all activity
        beads = fetch_recent_beads(4)
        prs = fetch_recent_prs(4)
        projects = fetch_active_projects()

        # Build combined table
        items = []
        for bead_id, title, status in beads:
            items.append((bead_id, f"[B] {title}", status))
        for pr_number, title, state in prs:
            items.append((pr_number, f"[PR] {title}", state))
        for project_id, name, state in projects:
            items.append((project_id, f"[P] {name}", state))

        if items:
            send_table(items)
            print(f"  sent {len(items)} items (beads={len(beads)} prs={len(prs)} projects={len(projects)})", file=sys.stderr)
        else:
            print("  no activity found", file=sys.stderr)

        time.sleep(args.every)


if __name__ == "__main__":
    main()
