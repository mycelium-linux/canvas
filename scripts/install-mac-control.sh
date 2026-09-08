#!/bin/bash
set -euo pipefail
if [[ "$(uname -s)" != Darwin ]]; then echo 'Dock Control requires macOS.' >&2; exit 1; fi
upgrade=false
case "${1:-}" in '') ;; --upgrade) upgrade=true ;; *) echo 'Usage: scripts/install-mac-control.sh [--upgrade]' >&2; exit 2 ;; esac
repo="$(cd "$(dirname "$0")/.." && pwd)"
app="$HOME/Applications/Dock Control.app"
if [[ -e "$app" ]]; then
  if [[ "$upgrade" != true ]]; then
    echo "Keeping installed helper unchanged: $app"
    /usr/bin/open -g "$app"
    exit 0
  fi
  if /usr/bin/pgrep -x canvas-control >/dev/null; then
    echo 'Quit Dock Control using Activity Monitor before an explicit upgrade. Restore reserved displays first.' >&2
    exit 1
  fi
fi
cd "$repo"
cargo build --release -p canvas-control
# Cargo owns where artifacts land: CARGO_TARGET_DIR, or build.target-dir in a cargo
# config, can point it away from ./target. Ask cargo rather than assume.
dock_target="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
mkdir -p "$HOME/Applications"
stage="$(mktemp -d "$HOME/Applications/.canvas-control.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
bundle="$stage/Dock Control.app"
mkdir -p "$bundle/Contents/MacOS"
cp -f "$dock_target/release/canvas-control" "$bundle/Contents/MacOS/canvas-control"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.futurepresentlabs.dock.control</string>
<key>CFBundleName</key><string>Dock Control</string>
<key>CFBundleExecutable</key><string>canvas-control</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
/usr/bin/plutil -lint "$bundle/Contents/Info.plist"
/usr/bin/codesign --force --sign "${DOCK_CONTROL_SIGN_IDENTITY:--}" --identifier com.futurepresentlabs.dock.control "$bundle"
/usr/bin/codesign --verify --strict "$bundle"
if [[ -e "$app" ]]; then
  backup="$app.backup.$(date +%Y%m%d%H%M%S)"
  if [[ -e "$backup" ]]; then echo "Backup already exists: $backup" >&2; exit 1; fi
  mv -f "$app" "$backup"
  echo "Previous helper retained at $backup"
fi
mv -f "$bundle" "$app"
/usr/bin/open -g "$app"
echo "Installed and launched $app"
echo 'Grant Accessibility to Dock Control in System Settings when requested by Dock.'
