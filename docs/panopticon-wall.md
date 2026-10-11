# Panopticon regional wall

Panopticon owns observations; Canvas owns presentation. This bounded companion
uses the existing public entity-near APIs on the Studio, reached through an
authorized private SSH connection. It does not use or weaken the authenticated
`/operator/spatial/snapshot` endpoint, which currently requires an unavailable
internal key. No credentials enter the scene or Lua binder.

The query is centered on Puget Sound (47.65, -122.35), radius 500 km. ADS-B uses
Studio loopback port 18080; AIS uses 18581. A successful empty AIS response is
reported as empty, not disconnected. HTTP/SSH errors are disconnected. Entity
IDs, coordinates, source identities when supplied, observation times and owner
TTLs are preserved. Expired entities are excluded even when the fetch succeeds;
a fetch timestamp is not a new observation timestamp. Invalid positions and
duplicate owner IDs fail visibly. At most 512 markers are presented, with an
explicit omitted count; a projection exceeding the bounded Lua input budget
fails rather than silently trimming data.

The optional owner asset is Panopticon's `dataPackage/ss_legal/us_states.geojson`.
The connector verifies its declared SHA256, extracts Washington's actual state
boundary (3 rings, 72 points in the inspected revision), and enforces 128-path /
8192-point limits. This is not detailed terrain, coastline tiles, or fabricated
map geometry. The asset remains in an operator-private cache, not copied into
this repository. Inspected checksum:
`6f23ed91fce2c25d57c01d83225342ba40258e9571c486c3999bd827c80d193b`.

## Manual runbook

Build only the small standalone host crate, using an isolated cached target:

```sh
cargo check --manifest-path crates/canvas-widget-host/Cargo.toml --bin canvas-panopticon-projection
cargo test --manifest-path crates/canvas-widget-host/Cargo.toml --bin canvas-panopticon-projection
cargo build --manifest-path crates/canvas-widget-host/Cargo.toml --bin canvas-panopticon-projection
lua config/binders/tests/panopticon-wall-v1-test.lua
```

Fetch and compose one real observation:

```sh
canvas-panopticon-projection fetch ajmwagar@100.108.9.93 OWNER_ASSET SHA256
# Feed the output to:
canvas-widget-host config/binders/panopticon-wall-v1.lua
```

Inspect the intended registered Canvas endpoint. Replace the map and source
widget specs in `config/workspaces/panopticon-wall.json` with that composition,
then import through the revision-bound owner API with `activate=false` unless
the user explicitly selected this screen. Preserve other workspaces and menu.
The receiver must support shared GFX `geo_map`; copying the definition does not
prove renderer support. A map uses source-relative geometry, no arbitrary URLs.

Once imported, an explicitly finite companion can refresh the declared map:

```sh
canvas-panopticon-projection watch ajmwagar@100.108.9.93 OWNER_ASSET SHA256 \
  config/binders/panopticon-wall-v1.lua canvas-widget-host canvas-tvctl \
  192.168.2.97:5555 60
```

This performs at most 60 observations at a nominal five-second cadence, then
exits. Source requests have bounded response sizes and request deadlines. The
map uses transient `surface_snapshot`; the endpoint independently expires
delivery after 31 seconds for the declared 10-second refresh envelope (three
periods plus one second of jitter), while this finite companion normally polls
every five seconds. This allows a bounded slow source read without normal
cadence flashing stale. Source-card edits are revision-bound patches only while this workspace
is selected; the companion never activates a different workspace. A generation
conflict is a visible error, not permission to override another writer. There is
no permanent service or unattended scheduler. After it exits, the receiver must
show expired/stale delivery rather than claiming an always-live feed. Receiver
expiry is an actual owner policy, not an unsupported field invented by Lua.

## Initial proof

The inspected real sample at 2026-10-11T03:11:19Z contained 42 current ADS-B
observations, zero expired observations, and a responding empty AIS source.
This is evidence of the source and composition, not proof of TV rendering or
ongoing replication. Hardware rendering, advancing timestamps, and endpoint
readback must be verified after the compatible app is installed.

FireTV APK45 was installed by the coordinating owner and this workspace was
activated through its revision-bound API. The three-cycle finite hardware proof
delivered real source reads at 03:17:00, 03:17:05 and 03:17:10 UTC on 2026-10-11:
34 current ADS-B entities and an empty responding AIS source. Owner presentation
readback showed `geo_map`, 34 markers, `ready` and the actual final read timestamp.
The inspected screenshot `/private/tmp/panopticon-wall-reordered.png` showed
updated aircraft labels/positions and no erroneous no-data footer. Health edits
must precede the transient map snapshot: the installed app can recreate views
after a document edit. The companion exited normally after that proof; this is
not an always-running deployment or continuous replication claim. A separately
owned sustained connector remains a follow-up.
