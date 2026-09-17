# Unibus — one bus for every surface

*The name is the DEC UNIBUS bet, restated: one shared backplane where audio,
display, presence, sensor, and control modules are all just grant-bearing
slots — heterogeneous devices, one narrow protocol. Nothing on the bus knows
another module's internals.*

*Design contract, mirroring JARVIS master (`distributed-topology.md`,
`crates/jarvis-core/src/edge_transport.rs`, `crates/jarvis-router`,
`crates/jarvis-edge`). Like that document, this describes target deployment
and recovery behavior; it does not imply transport, storage, or failover is
already implemented here.*

## Problem

Several machines can run Canvas: macbook-neo (laptop), mac-studio (desk
displays), home-pi.local with a TV (living room). We need one brain to decide
**which screen shows which content**, driven by JARVIS Presence, and to route
commands to the right screen — the same way JARVIS already routes audio and
status to zones.

## The JARVIS model being mirrored

JARVIS master splits into central and edge with a narrow, authenticated bus:

- **Hub (`jarvis-router`)** runs on the brain machine (mac-studio), listens on
  loopback only. Remote nodes connect through SSH reverse tunnels. Framing is
  `[u32 BE length][JSON RouterEnvelope][optional binary payload]`. The
  envelope carries `installation_id`, `schema_version`, `producer_id`,
  `destination`, `target_screen`, `request_id`, `issued_at_unix_ms`, `ttl_ms`,
  `token`.
- **Grants, not discovery.** The hub config maps `producer_id` → token file,
  zones, capabilities, screen anchors. The first frame must be `Register`;
  claimed capabilities are checked against the grant (fail closed). Per-node
  bearer tokens; stale envelopes (TTL) are rejected.
- **Registry.** Heartbeats keep registrations alive (3 s send, 10 s TTL);
  dead nodes are pruned and must re-register. Selection is
  zone + capability with deterministic tie-break (longest-connected, then
  producer_id), or direct addressing by `target_screen` anchor
  (`select_for_screen`).
- **Presence is a zone-keyed latest-value store.** A `PresenceCapture` node
  calls `PublishPresence`; the hub stores the snapshot per zone and fans a
  compact `PresenceSummary` out to every live `Display` node in that zone.
- **Edge (`jarvis-edge`)** is dumb and small: it connects, registers its
  capabilities, pipes `CanvasCommand` / `PresenceUpdate` / `VoiceIdentity`
  frames into the line protocol of a spawned overlay/audio program, and
  streams PCM both directions.
- **Cross-repo contract hygiene.** `jarvis-core` keeps thin, versioned,
  `deny_unknown_fields` copies of Canvas wire shapes (`canvas_contract.rs`,
  `canvas_desired_state.rs`, `canvas_intent.rs`) instead of depending on
  Canvas crates. Each side validates against the shared published contract.

## Decision: a shared transport, not a Canvas bus

JARVIS's router is becoming a general house bus — audio, streams, video,
sensor data, and commands — so that some machines can be screen-only
(home-pi TV), some audio-only, some both. We call that bus **Unibus**.
Therefore:

1. The **wire contract is lifted into a shared transport crate**
   (`unibus`, extracted from `jarvis-core::edge_transport` in the
   JARVIS repo; proposed upstream, not forked here — *call, don't rebuild*).
   Generalizations required for the lift, versioned so v1 stays valid:
   - Capabilities become an **open set of namespaced strings**
     (`canvas.display`, `audio.out`, `presence.capture`, `sensor.*`) instead
     of a closed enum, so adding a capability never bumps the protocol.
   - Message types are namespaced per domain (`canvas.*`, `audio.*`,
     `presence.*`, `sensor.*`); unknown messages route by destination without
     the relay needing to understand the body.
2. **Canvas is a client of this bus, not a second hub.** Dock does not run a
   competing `canvas-hub`; the same router process carries Canvas traffic.
   Dock vendors a thin contract mirror in `canvas-core` (the inverse of
   `canvas_contract.rs` — same discipline JARVIS already applies to us) with
   golden-frame parity tests sourced from the shared crate.
3. **Desired-state, not command streams, for screen content.** Canvas already
   reconciles idempotently (`crates/canvas/src/reconcile.rs`,
   `DesiredWorkspaceState` with schema version + revisions). Distribution
   rides on that: the brain publishes a per-surface *desired state* with a
   monotonic revision; edges reconcile, late or duplicate frames are
   idempotent no-ops. One-shot `CanvasCommand`s remain only for
   momentary effects (indicator blinks) where TTL expiry is the correct
   failure mode.

## Topology (v1)

| producer_id | machine | capabilities | zones | screen anchors |
|---|---|---|---|---|
| `mac-studio-brain` | mac-studio | presence.capture, audio io, canvas.display | studio | `screen-studio-left`, `screen-studio-right` |
| `macbook-neo` | macbook-neo | canvas.display, audio io, presence.capture | laptop | `screen-neo` |
| `home-pi-tv` | home-pi.local | canvas.display, audio.out | living-room | `screen-tv` |

Anchors come from the renderer-neutral display identities Canvas already
computes (`crates/canvas/src/display.rs::DisplayInfo`), not from invented
labels. One Canvas instance owns its machine's node connection — the full app
is the display edge; the thin `jarvis-status-overlay` path stays as-is for
machines not running Canvas.

## Presence → content policy

Each display edge receives `PresenceUpdate` summaries for its zones
(already surfaced in Canvas as the `jarvis.presence.v1` contract and
`canvas/src/presence.rs`). Policy is evaluated **on the edge** against the
zone it renders, deterministic and local:

- **Occupied + known occupant** → personal workspace; identity gates which
  project surfaces and feed content may render.
- **Occupied + unknown** → shared/ephemeral surface; never another person's
  private history (mirrors the JARVIS session-scoping rule).
- **Empty** → ambient/screensaver; private surfaces are evicted, not hidden.
- **Stale or conflicting presence** → treated as *unknown*, which means
  reserved disclosure — never as "empty" (a dead sensor must not leak) and
  never as a licence to keep private content up.

Presence informs *what* renders and *where delivery is allowed*; it must not
rewrite the static view definitions. Transitions are debounced by the hub's
existing snapshot cadence; edges never see raw frames or per-detection events.

## Command routing

Brain-side producers (JARVIS Subconscious, Canvas MCP clients) address
content three ways, resolved by the hub registry exactly as `jarvis-router`
does today:

1. **By screen anchor** — `target_screen` set; delivered only to the live
   display node claiming that anchor. This is how "put the briefing on the
   TV" works.
2. **By zone + capability** — all live `canvas.display` nodes in the zone;
   used for room-scoped content.
3. **Broadcast** — installation-wide, reserved for schema/config fan-out.

No target resolves → the hub logs loudly and replies `success: false` to the
producer; a send that silently hits zero screens is a bug, not a state.
Canvas's local control surface (`canvas-control` operations, the MCP server)
gains an optional `screen` target; unset means the local machine, preserving
every existing caller unchanged.

## Layering and QoS classes

This is a bus, not a network. Each layer borrows what it should not build:

> **Edges are Rust** (or Lua where a Canvas host embeds it). Python is for
> throwaway test harnesses on the producer side — never for an edge process
> a machine relies on. The deployed edges are `jarvis-edge` (audio, display,
> mute) and `canvas-edge` (Canvas surfaces).

| Layer | Owner | What it provides |
|---|---|---|
| L0 reachability | Tailscale (externally owned) | tailnet identity, IP across machines/OSes |
| L1 transport | `unibus` | QUIC-first connection, envelope framing, grants, registry, QoS mapping |
| L2 contracts | domain crates (`canvas.*`, `audio.*`, `presence.*`, `sensor.*`) | typed versioned semantics, validation, reconcile |

- **L1 is QUIC (quinn) first, TCP fallback.** QUIC gives built-in TLS 1.3,
  stream multiplexing with no cross-stream head-of-line blocking, the
  unreliable-datagram extension (RFC 9221), and connection migration across
  the laptop's address changes. The TCP fallback carries the existing
  `[u32 BE length][JSON][payload]` framing unchanged; envelope semantics are
  identical over either. No route computation, gossip, multi-hop, or
  store-and-forward intelligence — "mesh" here means the tailnet, not the
  bus topology.
- **Three QoS classes, mapped once in L1:**
  - **Command** (`canvas.command`, control, desired-state): reliable ordered
    stream; TTL enforced at the edge; idempotent via revision reconcile, so
    retransmit and duplicate are no-ops; unresolvable target → loud `false`
    reply, never a silent drop.
  - **Stream** (audio PCM, sensor samples): latency wins over completeness —
    unreliable datagrams when the chunk fits the negotiated datagram size
    (chop to ≤ 10 ms, drop late), otherwise one-way streams the reader may
    reset. Stale bytes are discarded, never queued behind loss.
  - **Snapshot** (presence, node state): latest-value replaceable,
    fire-and-forget, TTL; stale decays to *unknown*, never to *empty*.
- **Guardrail: L1 stays dumb.** Anything that parses a message body belongs
  in an L2 contract. A new capability or domain must not require an L1
  release.
- **Effective QoS is class × visibility.** Streams cost real IO and real
  power, so the edge publishes its own attention state — `canvas.visibility.v1`
  (snapshot class: which surfaces/cells are actually on screen, focused, or
  occluded/hidden) — and *content producers subscribe to it*: an NVR/camera
  tile updates only while it is visible on some screen, pauses to a stale
  thumbnail the moment it scrolls off or a screensaver takes the zone;
  project feeds follow their surface. Audio is the exception: while a
  playback session is live it is hard-realtime and never visibility-gated
  (a speaker behind the TV still plays). Off-screen is not low-priority —
  it is *off*: absence of a pull is the pause mechanism, no extra teardown
  protocol needed.
- **Input sharing rides the same planes.** Synergy-class keyboard/mouse
  handoff (pointer crosses a screen edge → control follows to that
  machine) is an L2 domain (`input.share.v1`), not L1: the *edge that owns
  the visible screen* is the input target, visibility/anchor state says
  where the edge is, grants say who may inject. Linux injection is
  `uinput` (the Pi and Agora need no keyboard of their own); macOS
  injection is `CGEvent` + the existing permission-bearing Dock Control
  helper — Input Leap ships this exact pattern on macOS, so the platform
  is not a blocker, just permissioned. Adoption policy: prefer bridging an
  existing Input Leap-compatible wire protocol over inventing a new one
  (*call, don't rebuild*), with hub-mediated handoff so presence and
  takeover can revoke the cursor like any other grant.

## Transport and security invariants

- QUIC listener binds the tailnet interface only (plus loopback for the brain
  itself), protected by tailnet ACLs; TLS 1.3 is inherent to QUIC. The TCP
  fallback keeps the existing pattern: hub loopback-only, remote edges over
  SSH reverse tunnels.
- **Identity root is fpl-auth; proof is a short-lived JWT.** Nodes enroll as
  per-user devices on `sso.fpl.dev`, following the existing desktop-bridge
  pattern (enroll → device token → heartbeat → revoke in the user's device
  list). A node presents an fpl-auth-issued RS256 JWT whose claims carry the
  grant scope (installation, producer, zones/capabilities/anchors),
  validated **offline** against pinned/cached JWKS — by the hub, and by a
  peer in a `unibus.direct` session with no hub round-trip. `exp − iat ≤
  300 s`, clock skew ≤ 5 s, `alg: none`/unknown-critical rejected. The
  claim shape and freshness rules are contract code in
  `FuturePresentLabs/unibus` (`auth::NodeClaims`); nothing invents a second
  PKI.
- **Revocation is expiry + device revoke.** Grant removal (or revoking the
  fpl-auth device) plus the ≤ 300 s assertion TTL bounds stale access to
  that window; no CRL/online-revocation machinery in v1. Register
  re-validates the grant on every reconnect; a dead node cannot heartbeat
  back to life. Hub-issued artifacts (`unibus.direct.v1` descriptors,
  config fan-out) are hub-signed JWTs — same offline validation shape, `exp`
  is their TTL.
- `installation_id` + `schema_version` + TTL validation on every inbound
  envelope, both directions. Frames ≤ 16 MiB, binary payload size must match
  the header declaration.
- A disconnected edge stops receiving but its screens are simply absent from
  selection — the brain keeps operating, and routing degrades loudly, never
  silently to the wrong screen.

## Content plane: indexes, caches, LAN/WAN locality

Fastest-path/IO routing is a real goal — but it belongs to *different*
planes, not to the control bus. The hub hop for commands is a correctness
cost, not a routing cost: multipathing control traffic would re-create a
routing protocol for zero benefit. Four layers, one path choice each:

- **Transport:** QUIC already selects and migrates per-connection paths.
  We do nothing.
- **Control:** one authoritative hub hop. Latency is irrelevant; authority
  *is* the topology.
- **Live media** (mic→speaker, capture): hub-*authorized*, hub-*signaled*
  edge-to-edge sessions (`unibus.direct.v1`: hub-signed JWT descriptor naming
  peers, `exp` as lifetime) so latency-sensitive streams skip the hub hop
  without skipping the grant — both peers validate it offline; same
  revoke-by-expiry/takeover model as the OpenCode control gates.
- **Content objects** (images, video, files, model artifacts): the place
  "nearest holder" pays off. See below.

### Referral, not location

Large payloads never ride envelopes. Producers announce
`content.index.v1` facts — content hash (blake3), provenance, size,
holders — as ordinary latest-value L2 contracts. The bus carries
*references*; bytes are pulled from holders by the resolver. CIDs are an
optional alias for nodes that run IPFS, not a bus requirement.

### Declared topology, measured refinement

Grants gain `site` (`home-lan`, `wan`, `agora-lan`) and `role`
(`origin`, `cache`, `edge`). Resolution order is deterministic: local disk
→ site cache → origin → declared-holder ranking (same-site first, then
measured heartbeat RTT, then producer id). Measured RTT refines ranking
*within* a declared site, never *across* one — a WAN node with an
optimistic RTT sample cannot outrank the LAN. Indexes stay explicit-registry
(Dolt-grant style, fail-closed); IPFS's gossipy DHT discovery is confined to
the optional IPFS backend, where a dead daemon degrades to origin fetch,
never to loss. This is the MeshCore lesson taken and left: heterogeneous
transports under one envelope, and site-named zones — but not autonomous
relay meshes, which is where routing re-invention creeps in.

### Pull-through caches

Each site's brain node caches objects it fetched for its zones; caches are
disposable and checksum-verified (corruption means refetch, not incident),
reads serve from cache, writes go through origin. This is the hub's presence
store pattern applied to bytes: latest value, TTL, stale → unknown → origin.

## Implementation boundary and tests

Dock-side deliverables (tracked in marbles):

1. `canvas-core::unibus` — envelope/frame codec + open-capability constants;
   golden-vector parity tests against `unibus` fixtures.
2. Edge connector in the Canvas app: register/heartbeat/reconnect with
   backoff; decode `canvas.*` + presence frames into app events; feed the
   reconciler. Unsupported/unconfigured targets report *unavailable*, never
   fake success.
3. Presence→policy evaluator per zone (pure, table-driven, unit-tested
   including stale/unknown/empty matrix).
4. Screen addressing on the control socket + MCP (`screen` target, default
   local, reject unknown anchors).
5. fpl-auth device enrollment + JWT assertions: offline validation,
   expiry-only revocation; `unibus.direct.v1` descriptor mint/verify.
6. Content plane: `content.index.v1` announce/resolve, deterministic
   nearest-holder ranking, disposable verified site caches.
7. Deployment units: hub config on mac-studio, edge units on macbook-neo
   (launchd) and home-pi (systemd) with per-node key provisioning script.

Crate-level tests establish codec round-trips, grant/capability enforcement,
registry selection determinism, staleness policy, and reconcile idempotency.
Deployment readiness additionally requires real partition/restart checks on
all three machines — passing unit tests do not imply the mesh is live.

## Non-goals

- No elected multi-hub failover in v1 (one authoritative hub per
  installation, same lease-restriction as JARVIS).
- No online revocation infrastructure (CRL/OCSP shape); expiry + grant edit
  is the v1 answer.
- No DHT/probabilistic discovery or autonomous relay in the bus; IPFS-style
  gossip is confined to the optional content backend.
- No bulk content bytes in envelopes — refs on the bus, pulls in the cache.
- No video pixel streaming over this bus; opaque desktop streaming stays on
  the existing Moonlight/Sunshine path (`computing-mesh-design.md` §3).
- No new presence sensing in dock — Presence owns sensing; Canvas consumes.
- Do not rebuild `jarvis-router` inside dock; lift the transport, share it.
