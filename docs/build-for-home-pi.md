# Building Canvas for home-pi (aarch64 Linux)

home-pi drives the TV (`screen-tv`) and holds `canvas.workspace_mirror`, so a
stale binary there means the TV silently stops following shared boards. It has
no Rust toolchain and 3 GB of RAM, so it cannot build for itself.

**agora-one is the build host.** x86_64, 12 cores, 30 GB, with the
`aarch64-unknown-linux-gnu` target and `aarch64-linux-gnu-gcc` already
installed.

## Why not cross-build from a Mac

`cargo-zigbuild` on macOS does work for the headless binaries — `canvas-edge`
cross-builds in about seven minutes. Prefer agora-one for `canvas` itself: it
links `libstdc++`, and a real GNU cross toolchain is less surprising than zig
for a C++-linking target.

The GUI binary needs no exotic sysroot. It links only `libc`, `libgcc_s`,
`libm` and `libstdc++` — graphics libraries are loaded at runtime, not link
time, so there is no need for aarch64 wayland/X11 development packages.

## One-time setup on agora-one

The workspace has two dependencies that do not travel with a plain checkout:

- `unibus` is a private git dependency pinned by rev, and agora-one has no
  GitHub SSH access. Copy the already-fetched cache instead of granting it
  credentials:

  ```sh
  for d in ~/.cargo/git/db/unibus-* ~/.cargo/git/checkouts/unibus-*; do
      rsync -a "$d/" "agora-one:~/.cargo/git/${d#$HOME/.cargo/git/}/"
  done
  ```

- `unibus-media` is a *relative path* dependency (`../../../unibus/...`), so
  both repos must sit side by side as `~/src/fpl/dock` and `~/src/fpl/unibus`.

## Sync and build

Anchor the excludes with a leading slash. `--exclude artifacts` also matches
`config/artifacts/`, which the canvas lib pulls in with `include_str!`, and the
build fails late with "couldn't read config/artifacts/…".

```sh
rsync -a --delete --exclude /target --exclude /.git \
    ~/src/fpl/unibus/ agora-one:~/src/fpl/unibus/
rsync -a --delete --exclude /target --exclude /.git --exclude /artifacts \
    ~/src/fpl/dock/ agora-one:~/src/fpl/dock/

ssh agora-one 'cd ~/src/fpl/dock && \
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
  CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
  cargo build --release -p canvas --bin canvas --features lua-scenes \
    --target aarch64-unknown-linux-gnu'
```

## Install

Keep the previous binary: a bad canvas leaves the TV with no way to draw, and
rolling back is faster than rebuilding.

```sh
scp agora-one:~/src/fpl/dock/target/aarch64-unknown-linux-gnu/release/canvas /tmp/canvas-new
scp /tmp/canvas-new ajm@home-pi.local:/tmp/canvas-new

ssh ajm@home-pi.local '
  file /tmp/canvas-new | grep -q aarch64 || { echo "wrong architecture"; exit 1; }
  sudo cp /usr/local/bin/canvas /usr/local/bin/canvas.prev
  sudo install -m 755 /tmp/canvas-new /usr/local/bin/canvas
  sudo systemctl restart canvas-compositor.service'
```

`canvas-edge` is a separate unit (`canvas-edge.service`) and is installed the
same way, but it can be cross-built on a Mac with
`cargo zigbuild --target aarch64-unknown-linux-gnu`.

## Verifying it took

Checking the binary is not enough — check that the TV follows a board:

```sh
ssh ajm@home-pi.local 'echo inspect | canvasctl'
```

Apply a document on another machine and confirm the surface count on all three
screens converges. A frozen `rev` on one screen means it is receiving nothing,
or rejecting everything.

## What a stale binary looks like

Both failures this procedure exists to fix were silent:

- `canvas-edge` panicking on `serde_json::Map` indexing, restarting 25,713
  times over two days. The TV simply never updated.
- `canvas` refusing any document carrying a field it did not know
  (`unknown field 'pages'`) — `deny_unknown_fields` means one new field rejects
  the whole board rather than the part it cannot draw. Until that changes, a
  document format change must reach **every** screen, including this one.
