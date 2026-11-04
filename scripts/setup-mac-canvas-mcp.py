#!/usr/bin/env python3
"""Install matching Canvas binaries and register its owner-local MCP adapter."""
from __future__ import annotations
import argparse, importlib.util, json, os, pathlib, plistlib, shutil, subprocess, tempfile, time

# Same-directory import: these installers are scripts run by path, not a package.
_spec = importlib.util.spec_from_file_location(
    "mcp_loopback", pathlib.Path(__file__).resolve().with_name("mcp_loopback.py")
)
mcp_loopback = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(mcp_loopback)

# The project is Canvas, but these identifiers name state already installed on a
# machine: a launchd label, a registered Desktop Bridge connector, and the paths
# they point at. Renaming them here silently orphans the installed copies rather
# than upgrading them, so they change in a deliberate migration, not a refactor.
LABEL = "dev.fpl.dock-canvas"
# The adapter is now a long-lived loopback server rather than a process the
# caller spawns, so it needs an agent of its own; the GUI agent stays as it is.
ADAPTER_LABEL = "dev.fpl.dock-canvas-mcp"
CONNECTOR_ID = "dock-canvas"
DEFAULT_PORT = 8084
INFO_PLIST = pathlib.Path(__file__).resolve().parents[1] / "crates/canvas/macos/Info.plist"
REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]

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

def install_runtime_config(support: pathlib.Path) -> None:
    binders = support / "binders"; views = support / "views"; themes = support / "themes/packs"
    binders.mkdir(parents=True, exist_ok=True); views.mkdir(parents=True, exist_ok=True)
    themes.mkdir(parents=True, exist_ok=True)
    for source in (REPO_ROOT / "config/binders").glob("*.lua"):
        shutil.copyfile(source, binders / source.name)
    for source in (REPO_ROOT / "config/themes").glob("*.toml"):
        shutil.copyfile(source, themes / source.name)
    for source in (REPO_ROOT / "config/views").glob("*.json"):
        document = json.loads(source.read_text())
        identity = document.get("id")
        if not isinstance(identity, str) or not identity:
            raise SystemExit(f"View has no id: {source}")
        parts = identity.split("-")
        if len(parts) < 2:
            raise SystemExit(f"View id must identify project and view: {identity}")
        destination = views / parts[0] / ("-".join(parts[1:]) + ".json")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)

def desired_connector(port: int, token: pathlib.Path) -> dict:
    """Register Canvas the way every desktop capability that works is registered.

    A `mcp-stdio` connector can only be reached by a caller that can spawn the
    process. FPL Desktop Bridge advertises a capability to the control plane
    after probing whatever serves it, and a remote route has to be proxied to an
    address, so the stdio registration stayed `configured`, was never
    advertised, and every cloud client asking for `dock.canvas-mcp` got the
    bridge's `requires_desktop_bridge` placeholder. Blender, Ghidra, the browser
    and the workspace adapter all worked because all four are loopback HTTP.
    """
    return {"localAuthTokenFile": str(token),
            "id": CONNECTOR_ID, "title": "Dock Canvas", "kind": "mcp-streamable-http",
            "enabled": True, "localUrl": f"http://127.0.0.1:{port}/mcp",
            "command": "", "args": [], "workingDir": "",
            "capabilities": ["canvas.inspect", "canvas.present"],
            "metadata": {"adapter": "dock-canvas-mcp", "app": "dock",
                         "capabilityId": "dock.canvas-mcp", "installManagedByFpl": "true",
                         "managedBy": "dock", "transport": "loopback-http"}}

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
    # Run the bundle executable directly so launchd owns one process. KeepAlive
    # restarts a crash but not a clean quit: closing the window must stay closed.
    return plistlib.dumps({"Label": LABEL,
        "ProgramArguments": [str(canvas_app / "Contents/MacOS/dock-canvas")],
        "EnvironmentVariables": {"HOME": str(home),
            "CANVAS_CONFIG_DIR": str(home / "Library/Application Support/Dock/canvas"),
            "PATH": f"{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"},
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "ThrottleInterval": 10,
        "ProcessType": "Interactive",
        "StandardOutPath": str(logs / "dock-canvas.log"),
        "StandardErrorPath": str(logs / "dock-canvas.err.log")}, sort_keys=True)

def adapter_agent(adapter: pathlib.Path, home: pathlib.Path, port: int) -> bytes:
    logs = home / "Library/Logs"; logs.mkdir(parents=True, exist_ok=True)
    # KeepAlive, unlike the GUI agent: the bridge probes this address to decide
    # whether to advertise the capability at all, so an adapter that stays down
    # after a crash silently un-publishes Canvas from every cloud client.
    return plistlib.dumps({"Label": ADAPTER_LABEL,
        "ProgramArguments": [str(adapter), "http", str(port)],
        "EnvironmentVariables": {"HOME": str(home), "PATH": f"{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"},
        "RunAtLoad": True, "KeepAlive": True, "ThrottleInterval": 3,
        "StandardOutPath": str(logs / "dock-canvas-mcp.log"),
        "StandardErrorPath": str(logs / "dock-canvas-mcp.err.log")}, sort_keys=True)

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--canvas", type=pathlib.Path)
    parser.add_argument("--adapter", type=pathlib.Path)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--no-launch", action="store_true")
    parser.add_argument("--sync-runtime-only", action="store_true",
                        help="reload views and Lua binders without rebuilding or restarting Canvas")
    args = parser.parse_args(); home = pathlib.Path.home()
    installed_canvas = home / ".local/bin/dock-canvas"
    installed_canvas_app = home / "Applications/DockCanvas.app"
    installed_adapter = home / ".local/bin/dock-canvas-mcp"
    canvas_support = home / "Library/Application Support/Dock/canvas"
    if args.sync_runtime_only:
        install_runtime_config(canvas_support)
        print(json.dumps({"schema_version": 1, "runtime_config": str(canvas_support),
                          "restarted": False}))
        return
    if args.canvas is None or args.adapter is None:
        parser.error("--canvas and --adapter are required unless --sync-runtime-only is used")
    config = home / "Library/Application Support/FPL Desktop Bridge/config.json"
    plist_path = home / f"Library/LaunchAgents/{LABEL}.plist"
    adapter_plist_path = home / f"Library/LaunchAgents/{ADAPTER_LABEL}.plist"
    token = mcp_loopback.token_path(home)
    if not args.no_launch:
        domain = f"gui/{os.getuid()}"
        subprocess.run(["launchctl", "bootout", domain, str(plist_path)], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["launchctl", "bootout", domain, str(adapter_plist_path)], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
    install_runtime_config(canvas_support)
    mcp_loopback.ensure_token(token)
    changed = register_connector(config, desired_connector(args.port, token))
    write_atomic(plist_path, launch_agent(installed_canvas_app, home), mode=0o644)
    write_atomic(adapter_plist_path, adapter_agent(installed_adapter, home, args.port), mode=0o644)
    if not args.no_launch:
        subprocess.run(["launchctl", "bootstrap", domain, str(plist_path)], check=True)
        subprocess.run(["launchctl", "bootstrap", domain, str(adapter_plist_path)], check=True)
        subprocess.run(["launchctl", "kickstart", "-k", f"{domain}/dev.fpl.desktop-bridge"], check=True)
    print(json.dumps({"schema_version": 1, "connector_id": CONNECTOR_ID, "config_changed": changed,
                      "port": args.port, "launched": not args.no_launch}))

if __name__ == "__main__": main()
