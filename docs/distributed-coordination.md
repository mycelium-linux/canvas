# Distributed Canvas Coordination

## Problem

A single human may have multiple Canvas-attached machines — a Mac desk dock, a
Pi in the living room, a TV, a laptop. Each runs its own `canvas-linux` or
`canvas-mac` instance with its own local control state. Today, control
ownership, project selection, and stream routing are all local-file-based and
single-machine. This document describes the path to coordinated multi-dock
operation while keeping everything optional and backwards-compatible.

## Existing single-machine primitives

The building blocks are already in place:

1. **`WorkspaceState`** (`canvas-core/src/lib.rs`) — a JSON file
   (`~/.dock/state.json` or `DOCK_STATE`) that tracks which project is selected,
   each project's lifecycle (Active/Idle/Paused), and who holds control
   (Human/Agent). It uses a crash lock (`state.lock`) for atomic read-modify-write.

2. **`Control::Gate`** (`canvas-linux/src/control.rs`) — a file-based ownership
   lock (`ownership.lock`) with epoch-based takeover. Every human revocation
   increments the epoch, so a delayed agent grant cannot undo a newer takeover.

3. **Stream sockets** (`canvas/src/stream_binding.rs`) — a Unix datagram socket
   (`stream.sock`) that accepts `StreamEnvelope` messages. Binders
   transform these into widget snapshots. The stream router
   (`config/stream-router.json`) routes events from sources (opencode,
   alfalfa, machine) to destinations (stream.sock, SSE).

4. **Control socket** (`canvas/src/control.rs`) — a separate Unix socket that
   accepts `ControlRequest` messages for document inspection, patches, surface
   placement, and workspace management.

## Distributed model

### Layer 1: State gossip (optional)

Each Canvas instance publishes its `WorkspaceState` to a local stream:

```
owner: "dock"
stream: "workspace-state"
contract: "canvas.workspace.state.v1"
payload: {
  "selected_project": "studio",
  "projects": {
    "dock": {"lifecycle": "idle", "control": "human"},
    "studio": {"lifecycle": "active", "control": "agent"}
  },
  "host": "home-pi.local"
}
```

A lightweight gossip relay (mDNS + UDP broadcast, or Tailscale DERP) forwards
these to other Canvas instances on the network. Each instance consumes the
published state but **does not act on it** — it only updates a read-only
presence table that the operator can inspect.

### Layer 2: Intent broadcast (optional)

When a human takes control or switches projects on one dock, that instance
broadcasts an intent event:

```
owner: "dock"
stream: "intent"
contract: "canvas.intent.v1"
payload: {
  "action": "select_project",
  "project_id": "studio",
  "host": "home-pi.local"
}
```

Other docks **may** mirror the selection (configurable per project), but this
is opt-in — a TV might follow the Pi's project, while a laptop stays independent.

### Layer 3: Control lease (optional, strong)

For true shared control (e.g., an agent on the Mac controlling a Pi's desktop),
the epoch-based ownership model extends to a distributed lease:

```
owner: "dock"
stream: "control-lease"
contract: "canvas.control.lease.v1"
payload: {
  "project": "dock",
  "host": "home-pi.local",
  "mode": "agent",
  "epoch": 3,
  "expires_at_ms": 1726123456789
}
```

The lease expires after a bounded TTL. A human takeover on any host revokes
all leases. This preserves the "fail loud, fail fast" principle — a dead host
doesn't hold control indefinitely.

## Now-playing widget: the simple case

You don't need any of this for a music display. The now-playing widget is:

1. **Local stream only** — `music-nowplaying-feed.py` sends to `stream.sock`
   on the same host. No network, no gossip.
2. **Binder transforms state into a widget snapshot** —
   `config/binders/music-nowplaying-v1.lua` maps MPD state to a `stat` or
   `gauge` widget.
3. **Workspace binds the stream** — a data binding maps
   `music.now-playing` → the widget's `snapshot` input.
4. **Idle tick is configurable** — set `motion.idle_tick_ms` to 1000 in
   `appearance.json` to drop the polling cadence from 30 FPS to 1 FPS. The only
   redraw happens when MPD state changes (stream event) or every 1 second
   (idle tick).

This works today with existing infrastructure. No distributed coordination
needed.

## Minimal deployment for home-pi

```
# On the Pi (ARM64 Linux)
cargo build --release -p canvas-linux --no-default-features --features desktop

# systemd unit for the music feed:
#   scripts/music-nowplaying-feed.py
#   --socket /run/user/1000/dock/canvas/stream.sock
#   --project dock --interval 2

# appearance.json (in config dir):
# { "motion": { "idle_tick_ms": 1000, "transition_ms": 0, "background_max_fps": 0 } }
```

The Pi runs `canvas-linux` for desktop/Sway management, the music feed script
publishes stream events, and Canvas (or a separate display surface) renders the
now-playing widget. The idle tick drops to 1 FPS, and MPD state changes trigger
immediate redraws via stream events.

## Future directions

- **mDNS service discovery** for Canvas instances on the local network
- **Stream bridge** that forwards Unix datagram events to a UDP multicast group
- **Distributed ownership** extending the epoch model across hosts
- **Presence-aware rendering** where a TV display dims when no one is looking

None of these are required for the now-playing widget. The stream socket, binder,
and appearance config are all local and sufficient.
