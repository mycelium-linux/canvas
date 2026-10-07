# Public Tardy reels on FireTV

The `Tardy` workspace shows a portrait public reel, its source caption, the
existing Neo/Codex agent dashboard, and the shared analog clock. Reels advance
automatically. The existing video mute button controls TV-local audio.

`canvas-tardy-tv` is an independently deployable Rust adapter. It reads the
anonymous `https://api.tardy.news/v1/feed?limit=100` endpoint and filters real
video reels. The authenticated reels endpoint is deliberately not used. There
are no Tardy credentials, account creation, posting, engagement writes or private
feed requests. `tardy.new` did not resolve during deployment; this uses the
working production API from the local Tardy checkout.

The feed response is bounded to 3 MiB, media to 100 MiB, and selection to 32
deduplicated reels of 1–300 seconds. Only explicit HTTPS Tardy media/R2 hosts
are admitted; HTTP redirects are disabled. Signed media URLs remain transient
and are not logged or saved into workspace documents. The adapter downloads
public MP4 bytes into its own temporary cache and transfers them to Agora.

Agora's adapter-owned `canvas-tardy-reel.service` remuxes the selected video
without video transcoding, encodes its audio as AAC, and publishes HLS through
the existing private, client-allowlisted media server. Its playlist is explicitly
`http://192.168.2.67:18089/tardy-public.m3u8`. Each playback generation changes the
Canvas URL to restart the decoder, including when revisiting the same post.
The adapter waits for a new playlist before switching presentation. It stops
only its own transient producer, never another media service.

Network failures update the caption panel to a visible degraded state. Reels
are polled/started only while this workspace is selected; leaving does not steal
selection or continuously download videos. A finite current relay may finish
after leaving. This is an initial sequential viewer, not the complete mobile
Tardy feed UI: there are no likes/comments, presence routing, or remote next/back
controls yet. Current selection follows the order of each anonymous feed read.

## Agent panel

`canvas-tardy-tv --agents SERIAL` consumes the existing private
`~/.local/share/agent-telemetry/agent-widget.json` source. It does not inspect
processes or infer another provider's state. It forwards a bounded transient
`surface_snapshot` every two seconds through `canvas-tvctl`; counts are not
saved as apparently live document edits. Fresh partial coverage stays partial,
and observations over 30 seconds old are stale. The Canvas endpoint also owns
receiver-side expiry when deliveries stop. This avoids the old retained-widget
watch path's `no_data` label for a transient status surface. This deployment
changes the Tardy panel only; the older Home panel updater is unchanged.

### Waiting for you and session details

The opt-in `agent.sessions.v2` presenter in
`config/binders/agent-dashboard-v2.lua` adds separate Needs input / Needs approval
counts and four priority-ordered session detail rows beneath the public caption.
Rows show safe project/task labels, owner/provider and time first observed waiting.
Unknown titles stay unnamed; no prompt, preview, transcript or full path is used.
Waiting sessions sort first, oldest observed wait first, then active/unknown/idle.
The analog clock and reel remain unchanged. These are read-only detail rows,
not controls that answer a question or approve an agent action.

The Lua presenter accepts authenticated snapshots/events, bounds owner and
session retention, rejects replay/invalid generations and ages old sessions to
unknown after 30 seconds. Old waits stop counting or displaying a growing timer.
Only Neo/Codex is connected in the current installation; the contract supports
other explicit owners but does not imply that their collectors are installed.

Use `config/deploy/firetv/agent-attention-runtime.example.json` with private token
and owner socket paths. Explicitly enable `--display-details` only for authorized
viewers; the V1 source and normalizer remain metadata-free by default.
On macOS, install the Lua consumer in the adapter's application-data directory,
not Documents: a newly signed launch-agent binary can block on a protected-folder
permission prompt. This installation uses
`~/.local/share/agent-telemetry/agent-dashboard-v2.lua`; copy the reviewed binder
there when deploying an update.

```sh
lua config/binders/tests/agent-dashboard-v2-test.lua config/binders/agent-dashboard-v2.lua
```

## Build and run

```sh
cargo check --manifest-path crates/canvas-tardy-tv/Cargo.toml --locked
cargo test --manifest-path crates/canvas-tardy-tv/Cargo.toml --locked
cargo clippy --manifest-path crates/canvas-tardy-tv/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path crates/canvas-tardy-tv/Cargo.toml --locked
canvas-tardy-tv 192.168.2.97:5555
canvas-tardy-tv --agents 192.168.2.97:5555
```

The two `config/deploy/neo/dev.fpl.canvas-tv-tardy*.plist` launch agents supervise
these modes. They require the existing SSH identity for `agora-one`, `ffmpeg`
there, and the already-authorized `canvas-tvctl`/ADB route. The source paths and
relay identity are explicit for this installation, not automatic discovery.

Merge `config/deploy/firetv/tardy.json` into the current catalog/documents with a
fresh revision-bound `canvas-tvctl` install; never replace unrelated workspaces.
The APK change is only the shared `canvas-menu.json` asset. No JNI/video rebuild
is needed for this adapter.
