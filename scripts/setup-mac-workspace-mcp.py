#!/usr/bin/env python3
"""Install Dock's authenticated loopback MCP and register it with FPL Desktop Bridge."""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import plistlib
import secrets
import shutil
import stat
import subprocess
import tempfile

LABEL = "dev.fpl.dock-mcp"
CONNECTOR_ID = "dock-workspaces"
CAPABILITY_ID = "dock.workspace-mcp"


def private_regular(path: pathlib.Path) -> bool:
    info = path.lstat()
    return stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and not info.st_mode & 0o077


def write_atomic(path: pathlib.Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = pathlib.Path(temporary_name)
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def ensure_token(path: pathlib.Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.parent.chmod(0o700)
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        if not private_regular(path) or len(path.read_text()) != 64:
            raise SystemExit("Existing Dock MCP token is unsafe or invalid")
        return
    with os.fdopen(descriptor, "w") as output:
        output.write(secrets.token_hex(32))
        output.flush()
        os.fsync(output.fileno())


def desired_connector(port: int, token: pathlib.Path) -> dict:
    return {
        "localAuthTokenFile": str(token),
        "id": CONNECTOR_ID,
        "title": "Dock Workspaces",
        "kind": "mcp-streamable-http",
        "enabled": True,
        "localUrl": f"http://127.0.0.1:{port}/mcp",
        "command": "",
        "args": [],
        "workingDir": "",
        "capabilities": ["workspace.inspect", "workspace.present", "presence.route"],
        "metadata": {
            "adapter": "dock-mac",
            "app": "dock",
            "capabilityId": CAPABILITY_ID,
            "installManagedByFpl": "true",
            "managedBy": "dock",
            "routing": "identified-presence",
        },
    }


def register_connector(config_path: pathlib.Path, desired: dict) -> bool:
    config = json.loads(config_path.read_text())
    connectors = config.get("connectors")
    if not isinstance(connectors, list):
        raise SystemExit("Desktop Bridge config has no connector list")
    existing = next((item for item in connectors if item.get("id") == CONNECTOR_ID), None)
    if existing == desired:
        return False
    if existing is not None and existing.get("metadata", {}).get("managedBy") != "dock":
        raise SystemExit(f"Connector collision: {CONNECTOR_ID} is not managed by Dock")
    if existing is None:
        connectors.append(desired)
    else:
        existing.clear()
        existing.update(desired)
    write_atomic(config_path, (json.dumps(config, indent=2) + "\n").encode())
    return True


def install_binary(source: pathlib.Path | None, destination: pathlib.Path) -> None:
    if source is None:
        if not destination.is_file() or not os.access(destination, os.X_OK):
            raise SystemExit(f"Dock executable is unavailable: {destination}")
        return
    if not source.is_file() or not os.access(source, os.X_OK):
        raise SystemExit(f"Built Dock executable is unavailable: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".new")
    shutil.copyfile(source, temporary)
    temporary.chmod(0o755)
    os.replace(temporary, destination)


def plist(binary: pathlib.Path, home: pathlib.Path, port: int) -> bytes:
    logs = home / "Library/Logs"
    logs.mkdir(parents=True, exist_ok=True)
    value = {
        "Label": LABEL,
        "ProgramArguments": [str(binary), "mcp-serve-http", str(port)],
        "EnvironmentVariables": {
            "HOME": str(home),
            "PATH": f"{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        },
        "RunAtLoad": True,
        "KeepAlive": True,
        "ThrottleInterval": 3,
        "StandardOutPath": str(logs / "dock-mcp.log"),
        "StandardErrorPath": str(logs / "dock-mcp.err.log"),
    }
    return plistlib.dumps(value, sort_keys=True)


def launch(label: str, plist_path: pathlib.Path) -> None:
    domain = f"gui/{os.getuid()}"
    subprocess.run(["launchctl", "bootout", domain, str(plist_path)], check=False,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["launchctl", "bootstrap", domain, str(plist_path)], check=True)
    subprocess.run(["launchctl", "kickstart", "-k", f"{domain}/{label}"], check=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=8083)
    parser.add_argument("--source-binary", type=pathlib.Path)
    parser.add_argument("--no-launch", action="store_true")
    args = parser.parse_args()
    if not 1024 <= args.port <= 65535:
        raise SystemExit("Port must be an unprivileged TCP port")

    home = pathlib.Path.home()
    installed = home / ".local/bin/dock-mac"
    support = home / "Library/Application Support/Dock"
    token = support / "mcp/token"
    bridge_config = home / "Library/Application Support/FPL Desktop Bridge/config.json"
    plist_path = home / f"Library/LaunchAgents/{LABEL}.plist"

    install_binary(args.source_binary, installed)
    ensure_token(token)
    register_connector(bridge_config, desired_connector(args.port, token))
    write_atomic(plist_path, plist(installed, home, args.port))
    if not args.no_launch:
        launch(LABEL, plist_path)
        subprocess.run(
            ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/dev.fpl.desktop-bridge"],
            check=True,
        )
    print(json.dumps({
        "schema_version": 1,
        "installed": True,
        "connector_id": CONNECTOR_ID,
        "capability_id": CAPABILITY_ID,
        "local_url": f"http://127.0.0.1:{args.port}/mcp",
        "launched": not args.no_launch,
    }))


if __name__ == "__main__":
    main()
