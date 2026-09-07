#!/bin/sh
# Install upstream multi-output transport beside Ubuntu packages; never restart sessions.
set -eu
sudo -n apt-get install -y meson ninja-build pkg-config libpixman-1-dev \
  libdrm-dev libgbm-dev libgnutls28-dev libturbojpeg0-dev libwayland-dev \
  libxkbcommon-dev libjansson-dev zlib1g-dev libpng-dev nettle-dev \
  libgmp-dev libpam0g-dev
python3 - <<'PY'
import hashlib, pathlib, tarfile, urllib.request
base = pathlib.Path.home() / '.cache/dock/wayvnc-build'
base.mkdir(parents=True, exist_ok=True)
sources = [
    ('aml', '1.0.0', 'b2b8f743213af39f40e8bc611147d69e2ea9e010b9b19cb65246582338f28d96'),
    ('neatvnc', '1.0.1', 'c37678fb1f9bbb8bd0932eb6dbd68bf10e937777c376c6c278ed37510b2cbd4b'),
    ('wayvnc', '0.10.1', '1dcb54f58d1637995bfb59c17709efca7833bae41f31b33eb47e608668a89d66'),
]
for project, version, digest in sources:
    archive = base / (project + '.tar.gz')
    if not archive.exists():
        urllib.request.urlretrieve(f'https://codeload.github.com/any1/{project}/tar.gz/refs/tags/v{version}', archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise SystemExit(f'{project}: source digest mismatch')
    with tarfile.open(archive) as source:
        source.extractall(base, filter='data')
    # Release archives have no repository metadata. Avoid Git discovery entirely.
    meson = base / f'{project}-{version}' / 'meson.build'
    meson.write_text(meson.read_text().replace(
        "git = find_program('git', native: true, required: false)", 'git = disabler()'))
PY
dock_vnc_prefix="$HOME/.local/lib/dock-wayvnc/0.10.1"
export PKG_CONFIG_PATH="$dock_vnc_prefix/lib/pkgconfig"
export LD_LIBRARY_PATH="$dock_vnc_prefix/lib"
cd "$HOME/.cache/dock/wayvnc-build"
for dock_vnc_project in aml-1.0.0 neatvnc-1.0.1 wayvnc-0.10.1; do
  set --
  if [ "$dock_vnc_project" = neatvnc-1.0.1 ]; then
    # TigerVNC uses Tight/ZRLE; no unused H.264 dependency chain.
    set -- -Dh264=disabled -Dtests=true
  fi
  meson setup "$dock_vnc_project/build" "$dock_vnc_project" \
    --prefix="$dock_vnc_prefix" --libdir=lib --buildtype=release "$@"
  meson compile -C "$dock_vnc_project/build"
  meson test -C "$dock_vnc_project/build"
  meson install -C "$dock_vnc_project/build"
done
mkdir -p "$HOME/.local/bin"
cat > "$HOME/.local/bin/dock-wayvnc" <<'SH'
#!/bin/sh
set -eu
dock_vnc_prefix="$HOME/.local/lib/dock-wayvnc/0.10.1"
export LD_LIBRARY_PATH="$dock_vnc_prefix/lib"
exec "$dock_vnc_prefix/bin/wayvnc" --output=HEADLESS-1 --disable-resizing --log-level=info "$@"
SH
chmod 755 "$HOME/.local/bin/dock-wayvnc"
"$dock_vnc_prefix/bin/wayvnc" --version
