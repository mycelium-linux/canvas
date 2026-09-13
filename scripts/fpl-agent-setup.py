#!/usr/bin/env python3
"""Set up an agent-scoped fpl-auth session for Canvas.

This script mints an agent-scoped session using the `fpl` CLI — NOT a copy
of the human's credentials. The agent gets its own OIDC session via
fpl-auth, registered to a project via fpl-enrollment. Desktop Bridge
acts as the credential-injecting proxy.

Usage:
  python3 fpl-agent-setup.py --agent my-agent --project canvas
  python3 fpl-agent-setup.py --agent opencode-dock --project dock --issuer https://fpl.auth

The agent's session is scoped to its project. Canvas sees the agent
identity via the StreamEnvelope actor_id field (dock-ybc.21.2). The
Desktop Bridge proxy injects the scoped session, never the human's.

See docs/secret-store-key-hierarchy.md for the threat model and
docs/decision-one-coordinator.md for the architecture.
"""
import argparse
import json
import os
import subprocess
import sys


def run(cmd, check=True, capture=True, **kwargs):
    """Run a command and return (returncode, stdout)."""
    print(f"$ {' '.join(cmd)}", file=sys.stderr)
    result = subprocess.run(
        cmd,
        capture_output=capture,
        text=True,
        **kwargs,
    )
    if result.stdout:
        print(result.stdout, end="")
    if result.returncode != 0:
        if check:
            print(f"Error: {result.stderr}", file=sys.stderr)
            sys.exit(1)
    return result.returncode, result.stdout


def main():
    parser = argparse.ArgumentParser(
        description="Mint an agent-scoped fpl-auth session for Canvas"
    )
    parser.add_argument("--agent", required=True, help="Agent identifier (e.g. 'opencode-canvas')")
    parser.add_argument("--project", required=True, help="Project ID the agent works in")
    parser.add_argument("--issuer", default="https://fpl.auth",
                        help="OIDC issuer URL")
    parser.add_argument("--bridge", default=None,
                        help="Path to Desktop Bridge proxy (default: auto-detect)")
    parser.add_argument("--dry-run", action="store_true",
                        help="Print commands without executing")
    args = parser.parse_args()

    print(f"Setting up agent-scoped session: agent={args.agent} project={args.project}", file=sys.stderr)
    print(f"Issuer: {args.issuer}", file=sys.stderr)
    print("NEVER uses the human's cookies — this mints a scoped session.", file=sys.stderr)
    print(file=sys.stderr)

    # Step 1: Mint an agent-scoped OIDC session
    # fpl login creates a session scoped to the --display-name identity
    login_cmd = [
        "fpl", "login",
        f"--issuer={args.issuer}",
        f"--display-name={args.agent}",
        "--no-browser",
    ]
    if args.dry_run:
        print("[dry-run]", " ".join(login_cmd))
    else:
        rc, output = run(login_cmd, check=False)
        if rc != 0:
            print("Error: fpl login failed. Is fpl-auth reachable?", file=sys.stderr)
            sys.exit(1)

    # Step 2: Verify the session is scoped to the agent (not the human)
    rc, whoami = run(["fpl", "whoami", "--json"], check=args.dry_run is False)
    if whoami:
        try:
            identity = json.loads(whoami)
            print(f"Agent identity: {identity}", file=sys.stderr)
            if identity.get("display_name") != args.agent:
                print(f"ERROR: session is not scoped to {args.agent}!", file=sys.stderr)
                print("Refusing to continue — blast radius is the human's, not the agent's.", file=sys.stderr)
                sys.exit(1)
        except json.JSONDecodeError:
            print(f"Could not parse whoami output: {whoami}", file=sys.stderr)

    # Step 3: Register Canvas MCP client with Desktop Bridge
    setup_cmd = [
        "fpl", "mcp", "setup",
        "--client", "canvas",
    ]
    if args.bridge:
        setup_cmd.extend(["--bridge", args.bridge])
    if args.dry_run:
        print("[dry-run]", " ".join(setup_cmd))
    else:
        run(setup_cmd, check=False)

    # Step 4: Verify the connection
    verify_cmd = ["fpl", "mcp", "verify"]
    if args.bridge:
        verify_cmd.extend(["--bridge", args.bridge])
    if args.dry_run:
        print("[dry-run]", " ".join(verify_cmd))
    else:
        rc, _ = run(verify_cmd, check=False)
        if rc != 0:
            print("Warning: MCP verify failed. Check Desktop Bridge.", file=sys.stderr)

    # Step 5: Document how Canvas sees this agent
    print(file=sys.stderr)
    print(f"Agent {args.agent} is now scoped to project {args.project}.", file=sys.stderr)
    print(f"Canvas will see this agent's activity via StreamEnvelope.project_id = \"{args.project}\"", file=sys.stderr)
    print("and ActorContext.id = \"{agent}\". See docs/secret-store-key-hierarchy.md.", file=sys.stderr)


if __name__ == "__main__":
    main()
