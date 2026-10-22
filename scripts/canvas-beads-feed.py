#!/usr/bin/env python3
"""Stream active Beads issues per project to Canvas.

Shows what's being worked on thematically — each project's active beads
as a table widget on the wall. This is the "sad activity table" made useful.
"""
import json, os, socket, subprocess, time

SOCKET_PATH = os.path.join(
    os.environ.get("HOME", ""), "Library/Application Support/Dock/canvas/stream.sock"
)
CONTRACT = "canvas.widget.snapshot.v1"

def main():
    if not os.path.exists(SOCKET_PATH):
        print(f"no Canvas stream socket at {SOCKET_PATH}", file=__import__('sys').stderr)
        return

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    seq = 0
    while True:
        try:
            # Get active issues grouped by project
            result = subprocess.run(
                ["bd", "list"],
                capture_output=True, text=True, timeout=10
            )
            lines = result.stdout.strip().split('\n')
            projects = {}
            current_project = None
            for line in lines:
                if line.startswith('○') or line.startswith('◐') or line.startswith('●'):
                    # Parse issue line like: ○ dock-cov ● P0 [bug] Canvas: ...
                    parts = line.strip().split()
                    if len(parts) >= 4:
                        bead_id = parts[0] if not parts[0].startswith('○') else parts[1]
                        project = None
                        for p in parts:
                            if p.startswith('dock-') or p.startswith('dock_cov'):
                                project = p.split('.')[0].replace('_','-')
                                break
                        if project and project not in projects:
                            projects[project] = []
                        if project:
                            projects[project].append(line.strip()[:60])

            # Build table rows
            rows = []
            for project, beads in sorted(projects.items()):
                rows.append([
                    project,
                    str(len(beads)),
                    "active" if beads else "idle",
                    beads[0][:40] if beads else "",
                    str(len(beads))
                ])

            seq += 1
            event = {
                "schema_version": 1,
                "owner": "beads",
                "stream": "beads-status",
                "contract": CONTRACT,
                "sequence": seq,
                "emitted_at_ms": int(time.time() * 1000),
                "payload": {
                    "limit": 100,
                    "running": sum(len(b) for b in projects.values()),
                    "queued": 0,
                    "total": sum(len(b) for b in projects.values()),
                    "jobs": [
                        {
                            "id": p, "state": "running" if beads else "idle",
                            "waited": len(beads), "ran": 0,
                            "cwd": p,
                            "command": "; ".join(beads)[:100] if beads else "no active work",
                            "note": f"{len(beads)} active beads"
                        }
                        for p, beads in sorted(projects.items())
                    ][:10]
                }
            }
            payload = json.dumps(event, separators=(",", ":")).encode()
            sock.sendto(payload, SOCKET_PATH)
        except Exception:
            pass
        time.sleep(10)

if __name__ == "__main__":
    main()
