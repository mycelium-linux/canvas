#!/bin/sh
# Run on Agora One from the Dock repository root. Preserve Sway and all apps.
set -eu
test "$(uname -s)" = Linux || { echo 'Run this script on Agora One.' >&2; exit 1; }
test -f config/linux/sway/config
dock_wayvnc="$HOME/.local/lib/dock-wayvnc/0.10.1-hotplug1/bin/wayvnc"
dock_libraries="$HOME/.local/lib/dock-wayvnc/0.10.1/lib"
dock_config="$HOME/.config/dock/wayvnc/config"
dock_state="$HOME/.local/state/dock/desktop.json"
test -x "$dock_wayvnc"
test -d "$dock_libraries"
test -f "$dock_config"
test -f "$dock_state"
python3 - "$dock_config" "$dock_state" <<'PY'
import json, os, pathlib, stat, subprocess, sys, time
config, state_path = map(pathlib.Path, sys.argv[1:])
info = config.lstat()
if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
    raise SystemExit("WayVNC configuration must remain a private regular file")
state = json.loads(state_path.read_text())
socket = state.get("sway_socket")
if not isinstance(socket, str) or not pathlib.Path(socket).is_socket():
    raise SystemExit("registered Sway socket is unavailable")
def sway(kind, *arguments):
    return subprocess.run(
        ["swaymsg", "-s", socket, "-r", "-t", kind, *arguments],
        check=True, text=True, capture_output=True,
    ).stdout
outputs = json.loads(sway("get_outputs"))
names = {output["name"] for output in outputs if output.get("active")}
if "HEADLESS-1" not in names:
    raise SystemExit("primary headless output is unavailable")
if "HEADLESS-2" not in names:
    result = json.loads(sway("command", "create_output"))
    if not result or not all(item.get("success") for item in result):
        raise SystemExit("Sway refused the second headless output")
    for _ in range(20):
        names = {output["name"] for output in json.loads(sway("get_outputs")) if output.get("active")}
        if "HEADLESS-2" in names:
            break
        time.sleep(0.1)
    else:
        raise SystemExit("second headless output did not appear")
commands = (
    'output HEADLESS-1 mode 1600x1000 pos 0 0; '
    'output HEADLESS-2 mode 1600x1000 pos 1600 0; '
    'workspace dock; move workspace to output HEADLESS-1; '
    'workspace studio; move workspace to output HEADLESS-2; workspace studio'
)
result = json.loads(sway("command", commands))
if not result or not all(item.get("success") for item in result):
    raise SystemExit("Sway refused the two-output project layout")
PY

install -m 644 config/linux/sway/config "$HOME/.config/dock/sway/config"
install -d -m 700 "$HOME/.config/systemd/user"
dock_unit="$HOME/.config/systemd/user/dock-wayvnc-headless-2.service"
if test -f "$dock_unit" && ! grep -q '^# Managed by Dock multi-monitor setup$' "$dock_unit"; then
    echo "Existing unit is not managed by Dock: $dock_unit" >&2
    exit 1
fi
cat > "$dock_unit" <<'UNIT'
# Managed by Dock multi-monitor setup
[Unit]
Description=Dock WayVNC transport for HEADLESS-2
After=dock-desktop.service

[Service]
Type=simple
Environment=XDG_RUNTIME_DIR=%t/dock-desktop
Environment=WAYLAND_DISPLAY=wayland-1
Environment=LD_LIBRARY_PATH=%h/.local/lib/dock-wayvnc/0.10.1/lib
ExecStart=%h/.local/lib/dock-wayvnc/0.10.1-hotplug1/bin/wayvnc --output=HEADLESS-2 --disable-resizing --log-level=info --config=%h/.config/dock/wayvnc/config --socket=%t/dock-desktop/wayvnc-headless-2 127.0.0.1 5902
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
UMask=0077

[Install]
WantedBy=default.target
UNIT
chmod 600 "$dock_unit"
systemctl --user daemon-reload
systemctl --user enable --now dock-wayvnc-headless-2.service
systemctl --user is-active dock-wayvnc-headless-2.service
python3 - <<'PY'
import socket, time
for attempt in range(30):
    try:
        with socket.create_connection(("127.0.0.1", 5902), timeout=1) as stream:
            if not stream.recv(12).startswith(b"RFB "):
                raise SystemExit("secondary WayVNC listener did not present RFB")
            break
    except OSError:
        if attempt == 29:
            raise SystemExit("secondary WayVNC listener did not start")
        time.sleep(0.1)
print('{"output":"HEADLESS-2","listen":"127.0.0.1:5902","rfb":"ready"}')
PY
