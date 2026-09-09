# Connecting canvas-sse to a real stream

What shipped: the mapping, the transforms, the emit loop, and the whole
subscription lifecycle above the transport — `SseSubscription` connects, frames,
resumes from the last event id, backs off when a stream ends, and reports which
of those is happening. `config/views/elmers-now.json`'s three `sse:` widgets stop
sitting on Loading — with no transport compiled in they report `disconnected`
with a reason, which is the honest state until something subscribes.

## What a follow-up has to supply

**1. One implementation of `Connector`.** The whole transport boundary is now two
methods:

```rust
fn connect(&mut self, topic: &str, last_event_id: Option<&str>) -> Result<Self::Stream, Unavailable>;
fn read(&mut self) -> Result<Vec<u8>, Unavailable>;   // Ok(vec![]) is open and quiet
```

Neither may block past `tick_seconds`, so the connection belongs on its own
thread with a bounded channel that `read` takes from. Everything else is already
here and already tested: `FrameBuffer` handles the wire format (multi-line
`data:`, `event:`, `id:`, comment keepalives, LF and CRLF) and keeps a partial
block for the next read; reconnect backoff, the attempt bound, the cooldown that
restarts the ladder, and the `Reconnecting` / `Unreachable` / `Unauthorized`
states a viewer sees are all above the trait. `SseSubscription::drain` is handed
`now_ms` rather than reading a clock, which is why every one of those states is
reachable in a test with no network.

Bounds to respect rather than re-invent: `MAX_EVENT_BYTES` per block,
`MAX_BUFFERED_BYTES` of unparsed stream, `MAX_FRAMES_PER_DRAIN` per tick,
`MAX_RECONNECT_ATTEMPTS` before `Unreachable`. `FrameBuffer::discarded` counts
what size refused; a transport that has somewhere loud to put that should.

**2. The brokered read, not a bearer.** `canvas-metrics` names an operation and
lets `fpl-desktop-bridge` attach the owner's credential (`metric-read`). SSE
needs the equivalent — a long-lived brokered subscribe the bridge holds open —
and this crate must keep holding no credential. Answer this before writing any
HTTP: if the bridge cannot proxy a stream, the connector needs a different
arrangement, not a bearer of its own. `Unavailable::Unauthorized` is already
wired to the `fpl login` re-auth path, on the same cooldown as `canvas-metrics`.

**3. Elmer's actual event contract.** `sse.example.json` guesses three topic
names (`events`, `attention`, `state`) and leaves `fields` empty, which falls
back to the built-in candidate keys (`subject`/`workflow`/`capability`/`name`/`id`,
`message`/`summary`/`detail`/`text`, `level`/`severity`/`outcome`/`state`,
`at_ms`/`timestamp`/`time`/`at`). Confirm topics and payload keys against
Elmer's SSE contract — `../elmers/src/ui.html` drives the same streams — and set
`fields` and `event_types` explicitly rather than relying on the fallbacks.
`docs/audits/canvas-operational-view-audit.md` also wants `elmers/brain` from
"the Elmer's and MCP SSE contracts"; that view needs a topology widget kind this
connector does not render yet.

**4. `config/sse.json` and a launch.** Copy `sse.example.json` to `config/sse.json`
and run `canvas-sse --config config/sse.json --socket <Canvas stream.sock>`
alongside `canvas-metrics`. Without `--socket` it prints envelopes to stdout,
which is the fastest way to see what a widget would receive.

## Decisions worth keeping or overturning deliberately

- **A snapshot is re-rendered whole from a bounded `Window`, every tick.** A
  subscription delivers increments, but a snapshot that is a delta breaks the
  moment one datagram is dropped — and datagrams are dropped, silently, by
  design. Re-rendering costs bytes and buys the property that any single
  snapshot is sufficient.
- **Stale keeps its body.** A failed Prometheus poll has nothing to show, so
  `canvas-metrics` falls back to a placeholder. A quiet subscription still holds
  real events, so it keeps them and marks the state `stale` with the newest
  event's own time as `observed_at`. Blanking would discard usable information.
- **Fitting shrinks entry count *and* text.** `canvas-metrics` only thins sample
  counts, which is enough for numbers. One 8KB log line overflows even a
  one-entry log, so the ladder shrinks both.
- **Reconnecting is a state, not a gap.** `Unavailable::Reconnecting` renders as
  `disconnected` — Canvas has no other tag for it, and inventing one would fail
  to deserialize and lose the whole snapshot — but its detail says the stream
  ended and something is being done. A viewer who cannot tell "reconnecting" from
  "never subscribed" cannot tell whether to go and look.
- **Exhausting the ladder is loud, not terminal.** `MAX_RECONNECT_ATTEMPTS`
  failures report `Unreachable` (a source error) and then wait
  `RECONNECT_COOLDOWN_MS` before starting the ladder again. A wall display that
  needs a human to restart it after a router blip is a worse failure than the
  blip; hammering a dead endpoint every tick is worse than both.
- **The burst is paced and a congested send is retried.** `AF_UNIX` datagrams
  have no flow control, so emitting every source back to back overruns the
  receive buffer and loses snapshots to `ENOBUFS` — and a lost snapshot leaves
  the widget on its previous value at `state: ready`, which reads as current.
  `SEND_SPACING` between sources and `send_retrying` for `ENOBUFS` alone; a
  failure that survives the retry is printed, never swallowed.
- **`stale_after_seconds` is declared, not inferred.** A subscription genuinely
  cannot tell "nothing happened" from "nothing arrived", so the operator says how
  long quiet is still healthy. This is the one place the crate takes a
  configured number over a derived one, and it is because no derivation exists.

## Duplication this pass created, and where it should go

`canvas-sse` depends on `canvas-metrics` for `MAX_DATAGRAM`,
`ENVELOPE_OVERHEAD`, `CONTRACT`, `STREAM_SCHEMA_VERSION`, `rfc3339` and
`placeholder`, because two copies of the datagram ceiling means one of them is
already wrong. That dependency edge is backwards: an SSE connector should not
depend on a Prometheus one. Those six items, plus `stream_identity` — which
exists in three places now (`canvas-metrics`, here, and `widget_source` in
`crates/canvas/src/operational_views.rs`, which is the rule the other two
mirror) — belong in a `canvas-stream` crate both connectors depend on, with
`stream_identity(transport, source_id)` taking the prefix as an argument.
`Unavailable` should not move as-is: the SSE vocabulary has `Disconnected`,
which a poller has no meaning for.
