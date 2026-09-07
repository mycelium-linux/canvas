#!/bin/sh
# Run on Agora after building the existing owner's desktop-feature binaries.
set -eu
owner_target=${1:?Usage: setup-agora-fpl-desktop.sh OWNER_TARGET_DIRECTORY}
test -x "$owner_target/fpl-desktop"
test -x "$owner_target/fpl-desktop-bridge"
test -x "$HOME/.local/bin/dock-linux"
app_root="$HOME/.local/lib/dock-fpl-desktop"
mkdir -p "$app_root/open" "$HOME/.local/bin"
install -m 755 "$owner_target/fpl-desktop" "$app_root/fpl-desktop.new"
mv -f "$app_root/fpl-desktop.new" "$app_root/fpl-desktop"
install -m 755 "$owner_target/fpl-desktop-bridge" "$app_root/bridge.new"
mv -f "$app_root/bridge.new" "$app_root/bridge"
# The GUI and generated client entries use this sibling owner CLI. SSH and
# project clients must reach the original keyring, not Sway's separate bus.
cat > "$app_root/fpl-desktop-bridge" <<'BRIDGE'
#!/bin/sh
set -eu
export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
exec "$HOME/.local/lib/dock-fpl-desktop/bridge" "$@"
BRIDGE
# Scope the URL handler to this app process. Never change GNOME's defaults.
cat > "$app_root/open/xdg-open" <<'BROWSER'
#!/bin/sh
set -eu
test "$#" -eq 1
exec "$HOME/.local/bin/dock-linux" desktop-web-url "${DOCK_FPL_PROJECT:?Project missing}" "$1"
BROWSER
cat > "$HOME/.local/bin/dock-fpl-desktop" <<'APP'
#!/bin/sh
set -eu
project=${1:?Usage: dock-fpl-desktop PROJECT [FPL_DESKTOP_OPTIONS]}
shift
# Validate the project and the live registered desktop through its owner.
"$HOME/.local/bin/dock-linux" desktop-status "$project" > /dev/null
test -S "${XDG_RUNTIME_DIR:?Run inside the Dock Sway desktop}/${WAYLAND_DISPLAY:?Wayland display missing}"
export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
owner_state=$(XDG_RUNTIME_DIR="/run/user/$(id -u)" systemctl --user show --property=ActiveState --value fpl-desktop-bridge-dock.service)
case "$owner_state" in
    inactive|failed) ;;
    *) echo 'The background FPL bridge owns this route; stop fpl-desktop-bridge-dock.service before opening the FPL app.' >&2; exit 1 ;;
esac
export DOCK_FPL_PROJECT="$project"
export PATH="$HOME/.local/lib/dock-fpl-desktop/open:$PATH"
# The GUI owns its daemon; a second GUI would contend for the same device route.
exec flock --nonblock --no-fork "$XDG_RUNTIME_DIR/fpl-desktop.lock" \
    "$HOME/.local/lib/dock-fpl-desktop/fpl-desktop" "$@"
APP
chmod 755 "$app_root/fpl-desktop-bridge" "$app_root/open/xdg-open" "$HOME/.local/bin/dock-fpl-desktop"
echo 'Installed FPL Desktop. From the project Sway desktop: ~/.local/bin/dock-fpl-desktop PROJECT'
