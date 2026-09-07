#!/usr/bin/env python3
"""Install matching Canvas binaries and register its owner-local MCP adapter."""
from __future__ import annotations
import argparse, json, os, pathlib, plistlib, shutil, subprocess, tempfile, time

LABEL = "dev.fpl.dock-canvas"
CONNECTOR_ID = "dock-canvas"
INFO_PLIST = pathlib.Path(__file__).resolve().parents[1] / "crates/dock-canvas/macos/Info.plist"

def write_atomic(path: pathlib.Path, data: bytes, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = pathlib.Path(name)
    try:
        os.fchmod(descriptor, mode)
        with os.fdopen(descriptor, "wb") as output:
            output.write(data); output.flush(); os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)

def install_binary(source: pathlib.Path, destination: pathlib.Path) -> None:
    if not source.is_file() or not os.access(source, os.X_OK):
        raise SystemExit(f"Built executable is unavailable: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".new")
    shutil.copyfile(source, temporary); temporary.chmod(0o755)
    os.replace(temporary, destination)

def install_canvas_app(source: pathlib.Path, app: pathlib.Path) -> None:
    executable = app / "Contents/MacOS/dock-canvas"
    install_binary(source, executable)
    shutil.copyfile(INFO_PLIST, app / "Contents/Info.plist")
    subprocess.run(["codesign", "--force", "--sign", "-", "--identifier",
                    "dev.fpl.dock.canvas", str(app)], check=True)

def desired_connector(adapter: pathlib.Path) -> dict:
    return {"id": CONNECTOR_ID, "title": "Dock Canvas", "kind": "mcp-stdio",
            "enabled": True, "localUrl": "", "localAuthTokenFile": "",
            "command": str(adapter), "args": [], "workingDir": "",
            "capabilities": ["canvas.inspect", "canvas.present"],
            "metadata": {"adapter": "dock-canvas-mcp", "app": "dock",
                         "capabilityId": "dock.canvas-mcp", "installManagedByFpl": "true",
                         "managedBy": "dock", "transport": "owner-local-socket"}}

def register_connector(config_path: pathlib.Path, desired: dict) -> bool:
    config = json.loads(config_path.read_text())
    connectors = config.get("connectors")
    if not isinstance(connectors, list): raise SystemExit("Desktop Bridge config has no connector list")
    existing = next((item for item in connectors if item.get("id") == CONNECTOR_ID), None)
    if existing == desired: return False
    if existing is not None and existing.get("metadata", {}).get("managedBy") != "dock":
        raise SystemExit(f"Connector collision: {CONNECTOR_ID} is not managed by Dock")
    if existing is None: connectors.append(desired)
    else: existing.clear(); existing.update(desired)
    write_atomic(config_path, (json.dumps(config, indent=2) + "\n").encode())
    return True

def launch_agent(canvas_app: pathlib.Path, home: pathlib.Path) -> bytes:
    logs = home / "Library/Logs"; logs.mkdir(parents=True, exist_ok=True)
    return plistlib.dumps({"Label": LABEL,
        "ProgramArguments": ["/usr/bin/open", "-n", "-a", str(canvas_app)],
        "EnvironmentVariables": {"HOME": str(home), "PATH": f"{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"},
        "RunAtLoad": True, "KeepAlive": False, "ProcessType": "Interactive",
        "StandardOutPath": str(logs / "dock-canvas.log"),
        "StandardErrorPath": str(logs / "dock-canvas.err.log")}, sort_keys=True)

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--canvas", type=pathlib.Path, required=True)
    parser.add_argument("--adapter", type=pathlib.Path, required=True)
    parser.add_argument("--no-launch", action="store_true")
    args = parser.parse_args(); home = pathlib.Path.home()
    installed_canvas = home / ".local/bin/dock-canvas"
    installed_canvas_app = home / "Applications/DockCanvas.app"
    installed_adapter = home / ".local/bin/dock-canvas-mcp"
    config = home / "Library/Application Support/FPL Desktop Bridge/config.json"
    plist_path = home / f"Library/LaunchAgents/{LABEL}.plist"
    if not args.no_launch:
        domain = f"gui/{os.getuid()}"
        subprocess.run(["launchctl", "bootout", domain, str(plist_path)], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # A pre-LaunchAgent development build may still own the control socket.
        subprocess.run(["pkill", "-x", "dock-canvas"], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(50):
            running = subprocess.run(["pgrep", "-x", "dock-canvas"], check=False,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if running.returncode != 0: break
            time.sleep(0.1)
        else: raise SystemExit("Existing Canvas did not stop")
    install_canvas_app(args.canvas, installed_canvas_app)
    install_binary(args.canvas, installed_canvas); install_binary(args.adapter, installed_adapter)
    changed = register_connector(config, desired_connector(installed_adapter))
    write_atomic(plist_path, launch_agent(installed_canvas_app, home), mode=0o644)
    if not args.no_launch:
        subprocess.run(["launchctl", "bootstrap", domain, str(plist_path)], check=True)
        subprocess.run(["launchctl", "kickstart", "-k", f"{domain}/dev.fpl.desktop-bridge"], check=True)
    print(json.dumps({"schema_version": 1, "connector_id": CONNECTOR_ID, "config_changed": changed, "launched": not args.no_launch}))

if __name__ == "__main__": main()
