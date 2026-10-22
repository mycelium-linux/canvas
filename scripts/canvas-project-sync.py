#!/usr/bin/env python3
"""Mirror all Alfalfa projects to Canvas as a status table.

Polls Alfalfa's /api/v1/projects endpoint and sends each project's status
as a UDP datagram to Canvas's stream.sock. The binder
(config/binders/project-status-v1.lua) renders it as a table widget showing
project name, status, owner, and resource count.

This is the "automatic pairing": Canvas discovers projects from the Alfalfa
index without any manual configuration. When a project's status changes,
the table updates on the wall.

Usage:
  canvas-project-sync --every 5       # poll every 5 seconds
  canvas-project-sync --once          # single snapshot
"""
import argparse, json, os, socket, sys, time, urllib.request

CONTRACT = "canvas.widget.snapshot.v1"
SCHEMA_VERSION = 1

def fetch_projects(alfalfa_url):
    """GET /api/v1/projects from Alfalfa and return a list of project dicts."""
    url = f"{alfalfa_url}/api/v1/projects"
    with urllib.request.urlopen(url, timeout=10) as resp:
        data = json.loads(resp.read().decode())
    projects = []
    for entry in data:
        proj = entry.get("project", {})
        projects.append({
            "id": proj.get("id", "?"),
            "name": proj.get("name", "?"),
            "purpose": proj.get("purpose", ""),
            "owner": proj.get("owner", "?"),
            "status": proj.get("status", "?"),
            "resources": entry.get("resources", 0),
            "repositories": entry.get("repositories", 0),
            "experiments": entry.get("experiments", 0),
        })
    return projects

def main():
    home = os.environ.get("HOME", "")
    default_socket = os.path.join(home, "Library/Application Support/Dock/canvas/stream.sock")
    default_alfalfa = os.environ.get("FPL_ALFALFA_URL", "http://127.0.0.1:8787")

    ap = argparse.ArgumentParser(description="Mirror Alfalfa projects to Canvas.")
    ap.add_argument("--socket", default=default_socket)
    ap.add_argument("--alfalfa", default=default_alfalfa)
    ap.add_argument("--owner", default="alfalfa")
    ap.add_argument("--stream", default="project-status")
    ap.add_argument("--every", type=float, default=5.0)
    ap.add_argument("--once", action="store_true")
    args = ap.parse_args()

    if not os.path.exists(args.socket):
        sys.exit(f"no Canvas stream socket at {args.socket} - start Canvas first")

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sequence = 0

    while True:
        try:
            projects = fetch_projects(args.alfalfa)
        except Exception as e:
            print(f"canvas-project-sync: cannot fetch from Alfalfa: {e}", file=sys.stderr)
            time.sleep(args.every)
            continue

        sequence += 1
        # Build a table: project name, status, owner, resources
        columns = ["Project", "Status", "Owner", "Resources", "Repos"]
        rows = []
        for p in sorted(projects, key=lambda x: x["name"]):
            rows.append([
                p["name"],
                p["status"],
                p["owner"],
                str(p["resources"]),
                str(p["repositories"]),
            ])

        payload = {
            "project": "alfalfa",
            "resources": projects,
        }
        event = {
            "schema_version": SCHEMA_VERSION,
            "owner": args.owner,
            "stream": args.stream,
            "contract": CONTRACT,
            "sequence": sequence,
            "emitted_at_ms": int(time.time() * 1000),
            "payload": {
                "limit": 256,
                "running": sum(1 for p in projects if p["status"] == "active"),
                "queued": 0,
                "total": len(projects),
                "jobs": [
                    {
                        "id": p["id"],
                        "state": p["status"],
                        "waited": 0,
                        "ran": 0,
                        "cwd": p["id"],
                        "command": p["purpose"][:80],
                        "note": f"resources={p['resources']} repos={p['repositories']}",
                    }
                    for p in projects
                ],
            },
        }

        body = json.dumps(event, separators=(",", ":")).encode()
        if len(body) > 64 * 1024:
            # Trim projects if too large
            event["payload"]["jobs"] = event["payload"]["jobs"][:10]
            body = json.dumps(event, separators=(",", ":")).encode()

        try:
            sock.sendto(body, args.socket)
            print(f"canvas-project-sync: {len(projects)} projects, {sequence} events", file=sys.stderr)
        except OSError as e:
            print(f"canvas-project-sync: send failed: {e}", file=sys.stderr)

        if args.once:
            return
        time.sleep(args.every)

if __name__ == "__main__":
    main()
