# Updating Canvas with Mycelium

Canvas owns the executable check, compositor dependency, service definition,
runtime configuration, and surface health. Mycelium carries a signed binary and
performs only the host-authorized swap, restart, check, and rollback.

Build and publish the ARM release:

```sh
env RUSTC_WRAPPER= cargo zigbuild --release \
  --target aarch64-unknown-linux-gnu \
  -p canvas --bin canvas --features lua-scenes
cp config/deploy/home-pi/mycelium/canvas.release-set.example.json \
  config/deploy/home-pi/mycelium/canvas.release-set.local.json
# Edit the local copy's version; keeping it here preserves relative paths.
mycelium releases publish-set \
  --manifest config/deploy/home-pi/mycelium/canvas.release-set.local.json \
  --signing-key /secure/mycelium-release.key --write
```

Install the reviewed policy once on `home-pi`. It remains local deployment
state and is never accepted from release gossip:

```sh
sudo install -d -m 0755 /etc/mycelium/components
sudo install -m 0644 \
  config/deploy/home-pi/mycelium/canvas.policy.json \
  /etc/mycelium/components/canvas.json
```

Inspect and activate:

```sh
mycelium update status --component canvas --channel canary
sudo --preserve-env=MYCELIUM_HOME,MYCELIUM_SOCKET \
  mycelium update apply --component canvas --channel canary \
  --policy /etc/mycelium/components/canvas.json --write
```

The candidate executes Canvas's `_self-check` without opening a window or
initializing the GPU. Mycelium then restarts `canvas-surface.service` and polls
its systemd state for up to twenty seconds. The compositor, Wayland display,
workspace document, observers, and Canvas protocols remain owned by Canvas and
its existing deployment configuration. A failed activation restores the prior
binary and verifies that the restored service is healthy.
