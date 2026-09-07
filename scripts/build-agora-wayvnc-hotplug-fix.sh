#!/bin/sh
# Build a separate candidate; never replace the running primary transport.
set -eu
dock_source_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
dock_base_prefix="$HOME/.local/lib/dock-wayvnc/0.10.1"
dock_candidate_prefix="$HOME/.local/lib/dock-wayvnc/0.10.1-hotplug1"
dock_candidate_source="$HOME/.cache/dock/wayvnc-hotplug/wayvnc-0.10.1"
test -f "$dock_base_prefix/lib/pkgconfig/neatvnc.pc"
python3 - <<'PY'
import hashlib, pathlib, tarfile
archive = pathlib.Path.home() / '.cache/dock/wayvnc-build/wayvnc.tar.gz'
assert hashlib.sha256(archive.read_bytes()).hexdigest() == '1dcb54f58d1637995bfb59c17709efca7833bae41f31b33eb47e608668a89d66'
base = pathlib.Path.home() / '.cache/dock/wayvnc-hotplug'
base.mkdir(parents=True, exist_ok=True)
with tarfile.open(archive) as source:
    source.extractall(base, filter='data')
meson = base / 'wayvnc-0.10.1/meson.build'
meson.write_text(meson.read_text().replace(
    "git = find_program('git', native: true, required: false)", 'git = disabler()'))
PY
patch --batch --forward -d "$dock_candidate_source" -p1 < "$dock_source_root/config/linux/patches/wayvnc-idle-hotplug.patch"
export PKG_CONFIG_PATH="$dock_base_prefix/lib/pkgconfig"
export LD_LIBRARY_PATH="$dock_base_prefix/lib"
meson setup "$dock_candidate_source/build" "$dock_candidate_source" \
  --prefix="$dock_candidate_prefix" --libdir=lib --buildtype=debugoptimized
meson compile -C "$dock_candidate_source/build"
meson test -C "$dock_candidate_source/build"
meson install -C "$dock_candidate_source/build"
"$dock_candidate_prefix/bin/wayvnc" --version
