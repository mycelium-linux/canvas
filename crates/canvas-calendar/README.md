# Canvas Calendar

A host-embedded egui agenda using the host's existing device, rendering pass,
style and account palette. No window, network calls, credentials or persistence.
`Snapshot::from_bridge` accepts `fpl-desktop-bridge calendar-read events` schema
1 observations. `Agenda::show` renders a virtualized list and selected context.

The host owns account enrollment, observations, page retrieval, ordering and
merging across calendars, freshness expiry, selected workspace and surface
lifecycle. Preserve notices for partial pages. Selection uses the exact
connection/calendar/event tuple; never join events by title or account alone.

Dates remain in the source offset/timezone. All-day end dates are exclusive.
This first view is an agenda, with no time-grid collision layout or write
actions. Malformed observations fail instead of silently hiding broken events.
The source does not by itself mount a Calendar view on the running Canvas host.

```sh
cargo check --manifest-path crates/canvas-calendar/Cargo.toml
cargo test --manifest-path crates/canvas-calendar/Cargo.toml
```

Headless tests exercise rendering and date semantics. Native GPU, WebGPU and
Fire TV input/rendering remain integration acceptance checks, not inferred from
these tests. The host's shared Coherence style and egui pass should be reused.
