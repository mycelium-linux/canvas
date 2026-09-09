# Widget quality bar

Canvas widgets are read in passing, on a wall, next to seven others. A widget
that is merely correct but hard to read costs attention every time someone
glances at it, and a widget that is wrong in a way you cannot see costs more
than that.

This is the bar every widget in `canvas-widgets` is held to. Each rule is
stated so it can be checked mechanically or by reading one function, not
argued about. Where a rule is currently violated, the violation is named.

## 1. Provenance is always visible

A widget may render synthetic data. Designing a surface before its source
exists is legitimate and useful. What a widget may never do is render synthetic
data that looks like real data.

Provenance is carried next to the data state, not inside it, because a demo
widget can be perfectly `Ready`:

```rust
pub enum Provenance { Live, Demo, Sample }
```

`Demo` and `Sample` render with a persistent corner badge and a distinct accent,
present in every state including `Ready`. The badge is not dismissible, and it
is not a tooltip: someone walking past must see it.

> **Violation.** `canvas-widgets/src/live.rs:22` `mock_price()` hashes a symbol
> into a price and a change, and `ticker_text` renders `AAPL $437.82 (+ 3.2%)`
> with nothing marking it as invented. Fabricated financial figures that read as
> quotes are the most damaging form this can take.

## 2. Freshness is always visible

`Stale` must not render identically to `Ready`. A value ten minutes old and a
value from this second look the same today, which quietly turns a dashboard
into a liar during exactly the incident it exists for.

Stale keeps the body — a stale number is still information — but carries a
visible freshness treatment and an observed-at time.

> **Violation.** `canvas-widgets/src/live.rs:622` gates
> `Loading | NoData | Disconnected | Error` into a status treatment and lets
> `Ready | Stale | Partial` fall through to the identical body.

## 3. Absent data says why

`NoData` states a reason in the widget's own words: "Bifrost exposes no cache
metric", not an empty panel or a zero. A blank widget is indistinguishable from
a broken one, and a zero is worse than blank because it is a claim.

This one is met by the metrics connector today (`canvas-metrics`), which
declares unbacked sources with a reason rather than emitting a fabricated zero.

## 4. Named for shape, not for use

A widget kind names what it draws. The domain lives in the binder that feeds it
and the title the view gives it.

`NodeSummaryItem` is `{ label, state, detail }` — a labeled status list.
`BurnRateWindow` is `{ label, burn_rate }` — labeled rates against an
objective. Both are already generic; only their names are not, and a name that
describes one use discourages the other nine.

Specialization is still allowed where the shape genuinely is the domain:
candlestick charts carry open/high/low/close because that *is* the structure.
The test is whether the payload would be contorted to serve a second use, not
whether a second use is currently imagined.

> **Rename:** `NodeSummary` → `StatusList`, `SloBurnRate` → `RateBars`.
> **Keep:** `Quant`, which is domain-shaped by nature.

## 5. A widget does not source its own data

A widget receives data. It does not read files, open sockets, fetch, or read
the clock. Anything a widget can fetch, it can fetch at the wrong time, in the
wrong order, with credentials it should not hold.

> **Violations.** `Chart { source: PathBuf }` holds a filesystem path.
> `Ticker { symbols }` carries identifiers it resolves to values itself.
> `Clock { format }` reads the system clock — intrinsic rather than sourced, and
> the one defensible exception.

## 6. Every collection is bounded

Every `Vec` in a payload has a documented cap and a defined overflow behavior:
thin, truncate with a disclosure, or refuse. Unbounded collections are a denial
of service against the renderer, and the ceiling that matters is rarely the
obvious one.

> A snapshot fitted to Canvas's 64KB `MAX_STREAM_MESSAGE` still failed with
> `EMSGSIZE`, because macOS caps an `AF_UNIX` datagram at
> `net.local.dgram.maxdgram` — 2048 bytes. Cap against the limit that binds,
> and prove it with a test over the whole envelope, not the widget body.

## 7. Legible at a glance

The one axis that is judgement rather than a check, so it gets explicit
constraints instead of adjectives:

- One type scale across the library. A widget does not invent a size.
- Body text and every value meets WCAG AA against its own background in both
  light and dark themes.
- A number is readable from across a room: values dominate, labels recede.
- Units and magnitude are never implied. `23.6 W`, not `23.6`.
- Eight widgets on one screen read as one system, not eight cards.

## The artifact model

A widget is not useful alone. What is useful is a query, a mapping, and a
presentation that agree with each other — today those live in three unrelated
files with nothing tying them together:

```
config/metrics.json   the query          →  what to ask for
config/binders/*.lua  the mapping        →  how the result becomes a widget
canvas-widgets        the presentation   →  how the widget draws
```

A **widget artifact** binds them into one addressable thing:

```
artifacts/gpu-fleet/
  manifest.json     id, version, widget kind, source binding, provenance
  binder.lua        the mapping, sandboxed and bounded as binders already are
```

Resolution is project first, then global, so a project can override a shared
artifact without forking it:

```
<project>/artifacts/<id>/   →  ~/.../Dock/canvas/artifacts/<id>/  →  built-in
```

This is what makes consistency a default rather than a discipline. Pulling up
`gpu-fleet` brings the query, the mapping, and the presentation as a unit, and
the quality bar above is enforced once at the artifact boundary instead of
being re-litigated per widget.

## What is checked where

| Rule | Enforcement |
|---|---|
| 1 Provenance | type system: `Provenance` is non-optional on `WidgetData` |
| 2 Freshness | render test: `Stale` output differs from `Ready` |
| 3 Absent says why | render test: `NoData` renders its `detail` |
| 4 Shape names | review, at artifact boundary |
| 5 No self-sourcing | `canvas-widgets` does not depend on `std::fs`, `reqwest`, or a clock |
| 6 Bounded | test per kind over the full envelope against `MAX_DATAGRAM` |
| 7 Legible | contrast test over the palette matrix; type scale is a shared constant |

Rules 1, 2, 3, 5 and 6 are mechanical. 4 and 7 need a person, which is why they
are the two worth spending review time on.
