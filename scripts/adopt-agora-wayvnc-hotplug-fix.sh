#!/bin/sh
# Explicit adoption; preserve old launcher and do not restart the compositor.
set -eu
case "${1:-}" in ''|--restart) ;; *) echo 'usage: adopt-agora-wayvnc-hotplug-fix.sh [--restart]' >&2; exit 2;; esac
dock_base_prefix="$HOME/.local/lib/dock-wayvnc/0.10.1"
dock_candidate_prefix="$HOME/.local/lib/dock-wayvnc/0.10.1-hotplug1"
LD_LIBRARY_PATH="$dock_base_prefix/lib" "$dock_candidate_prefix/bin/wayvnc" --version
dock_launcher="$HOME/.local/bin/dock-wayvnc"
if [ ! -e "$dock_launcher.pre-hotplug1" ]; then
  cp -f "$dock_launcher" "$dock_launcher.pre-hotplug1"
fi
cat > "$dock_launcher.next" <<'SH'
#!/bin/sh
set -eu
export LD_LIBRARY_PATH="$HOME/.local/lib/dock-wayvnc/0.10.1/lib"
exec "$HOME/.local/lib/dock-wayvnc/0.10.1-hotplug1/bin/wayvnc" \
  --output=HEADLESS-1 --disable-resizing --log-level=info "$@"
SH
chmod 755 "$dock_launcher.next"
mv -f "$dock_launcher.next" "$dock_launcher"
if [ "${1:-}" = --restart ]; then
  systemctl --user restart dock-wayvnc
  systemctl --user is-active dock-wayvnc
else
  echo 'Launcher adopted; restart only dock-wayvnc when ready.'
fi
