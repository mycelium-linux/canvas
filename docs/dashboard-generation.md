# Generating dashboards from a Prometheus job

Today a new service becomes a Canvas dashboard by hand: someone writes PromQL
into `config/metrics.json`, picks a widget kind, guesses a unit, and places
rectangles in `config/views/*.json`. `config/metrics.json` currently holds 21
sources and 11 of them are backed by a metric that exists. The other 10 are
declared views of things nobody exposes, and the only reason they are not
permanently `Loading` is that someone remembered to write an `unavailable`
reason next to each one.

That does not scale past a handful of jobs, and it drifts the moment a service
adds a metric.

This document describes the pipeline that replaces it, and specifies the
**candidate widget definition** — the artifact the survey emits and a human or
an LLM consumes. It is a format specification, so every field below is stated
so it can be checked rather than argued about.

The failure this whole effort exists to remove is a widget that says `Loading`
forever because its query returns nothing. Every rule here traces back to that.

## The pipeline

| # | Stage | Actor | In | Out |
|---|---|---|---|---|
| 1 | Enumerate | code | Prometheus metadata endpoints | metric names, types, HELP, units, label cardinality |
| 2 | Classify | code | that inventory | a candidate per rule that fires, plus four accounting lists |
| 3 | Verify | code | each generated query | executed result, or the candidate is demoted |
| 4 | Emit | code | verified candidates | `<subject>.candidates.json` |
| 5 | **Select** | **human or LLM** | candidates + a statement of what the subject is | `<subject>.selection.json` |
| 6 | Lay out | code | selection + `size_hint` + display count | `config/views/<id>.json` |
| 7 | Install | code | selection | merged sources in `config/metrics.json` |

Stage 5 is the only one a model touches, and it is the only one where the
question has no correct answer.

## Why the seam falls there

**Classification is code because it is deterministic and testable.** Given a
metric named `invoker_errors_total`, a `TYPE counter` from
`/api/v1/metadata`, and a small label set, the widget kind and the query
shape follow with no judgement at all: a counter is a `rate()` over a window
rendered as a time series, because a raw counter is a monotonic ramp that tells
you nothing. That rule either fires correctly or it does not, and a test can
say which. Ask a model to do it and you get a different answer on Tuesday, no
way to regression-test the difference, and a fabricated query when the model
would rather be helpful than empty-handed.

**Selection is a person or an LLM because it is taste.** Nothing in the metric
inventory says whether GPU memory headroom matters more than active SSO
sessions on a wall someone walks past. That is a claim about what this project
is and what breaks it, and it is not recoverable from metric names, HELP text,
or cardinality. A rule that tried would be a hard-coded opinion pretending to
be a derivation.

**Layout is code again because it is arithmetic.** `canvas-widgets::size_hint`
already returns, per kind and derived from the theme's type scale, a minimum
below which the content stops being readable and a comfortable size a layout
should aim for, plus a preferred aspect. Display count comes from the platform
(`dock-blw`). Packing an ordered list of kinds into that space is a solved
arithmetic problem with a right answer, and asking a model to place rectangles
produces overlapping ones.

The model sits in exactly one place: the place where taste is required.

## Stage 1: what Prometheus already tells us

Three read-only endpoints supply everything classification needs:

| Endpoint | Supplies |
|---|---|
| `/api/v1/metadata` | `type` (counter, gauge, histogram, summary), `help`, `unit` |
| `/api/v1/label/__name__/values` | the metric names present |
| `/api/v1/series` | label sets per metric, hence cardinality |

HELP is a free widget title. `type` is the single most load-bearing input.
Cardinality decides whether a metric is one number, a small breakdown, or a
matrix.

> **Blocked, and it is small.** `fpl-desktop-bridge/src/metric_reads.rs` allows
> `query`, `query_range` and `/-/ready` only. These three routes are read-only
> and belong in the same allowlist, added the same way — an enumerated route
> each, never an arbitrary path passthrough.

## Stage 2: classification rules

Evidence is `(type, series count, name suffix, family membership)`. Nothing
else. Each rule below names the evidence that fires it, and every candidate
records which rule fired, so a reviewer corrects a wrong guess instead of
re-deriving it.

### Single-metric rules

Tested in table order; the first rule that fires wins, and the candidate records
which one it was.

| Evidence | `source.kind` | Query shape |
|---|---|---|
| `_info` | none — **excluded** | — |
| histogram (`_bucket`) | `time_series` per quantile in {0.5, 0.9, 0.99} | `histogram_quantile(q, sum by (le) (rate(m[5m])))` |
| counter (`_total`), any cardinality | `time_series` | `sum by (<label>) (rate(m[5m]))` |
| gauge in `0..1`, or `_ratio` | `gauge` with `scale: 100` | `m` |
| gauge, 1 series | `stat` | `m`, or an aggregation |
| gauge, 2–12 series | `ranked_bar` | `m` |
| gauge, >12 series | `heatmap`, or top-N `ranked_bar` with the tail disclosed | `m` |

The counter rule is most of what separates a useful generator from a naive one.
A dashboard of raw counters is a dashboard of straight lines.

The ratio rule sits above the cardinality rules on purpose: a gauge already
bounded to `0..1` is a proportion, and rendering a proportion as a bare `stat`
reading `0.83` throws away the one thing that makes it legible — that it has a
ceiling.

`_info` metrics are label enrichment for other metrics, never a widget. They go
in `excluded` with that reason, not silently nowhere.

### Family rules

Detected before single-metric rules, because a family produces one good widget
where its members produce two bad ones.

| Family | Emits |
|---|---|
| `X_used_bytes` + `X_bytes` | one utilisation `gauge`, `100 * sum(used) / sum(total)` |
| `X_total` + `X_errors_total` | one error-ratio `time_series`, not two counters |
| `X_sum` + `X_count` | one average `time_series`, `rate(sum) / rate(count)` |
| longest common prefix | a panel group hint in `evidence.family`; not itself a widget |

`config/metrics.json` already hand-encodes the first of these for VRAM. That is
the point: the rule is a derivation of something a person already did by hand
and will otherwise have to keep doing.

It also shows why the derivation is worth having. `config/metrics.json` spells
the Yggdrasil metrics two ways — `yggdrasil_accelerator_memory_used_bytes` in
`fleet/vram`, `bifrost_yggdrasil_runtime_inflight_requests` in `fleet/inflight`
— and only Prometheus knows which prefix is current. A survey reads `__name__`
and cannot be wrong about it; a person retyping a metric name can.

A family rule fires only when **every** member it needs is present. A lone
`X_errors_total` with no sibling `X_total` falls through to the counter rule and
produces an error *rate*, not an error *ratio*. The ratio is the better widget
and the survey must not pretend to have it.

### Units and scale

`SourceSpec.scale` exists so a unit conversion is data rather than PromQL:

| Suffix | `scale` | `unit` |
|---|---|---|
| `_seconds` | `1000` | `ms` |
| `_bytes` | `9.3132257461548e-10` (1/2³⁰) | `GiB` |
| `_ratio`, gauge in `0..1` | `100` | `%` |
| `_total` after `rate()` | `1` | `<noun>/s` |

`scale` is a number, not an expression: write the reciprocal, not the division.

The emitter's `gauge` arm emits `value` and `label` and ignores `unit`, so a
`unit` set on a gauge source is inert and the unit must go in `label`. This is
the one place where the source mapping and the widget disagree about where a
unit lives, and it is why the violation below exists.

### Titles

A title states what is measured and its unit, and implies no magnitude it does
not have (`docs/widget-quality.md` rule 7). Generated titles follow three rules:

1. The unit appears **exactly once** in a spec: in the `unit` field for kinds
   that have one (`stat`, `time_series`, `histogram`, `heatmap`, `ranked_bar`,
   `stacked_bar`), and in the title or label for kinds that do not (`gauge`,
   `state_timeline`, `node_summary`, `table`, `event_log`, `slo_burn_rate`).
2. HELP is trimmed to a noun phrase, never pasted as a sentence. Absent HELP
   falls back to the family name in words.
3. A window or an aggregation that narrows the claim appears in the title:
   `p99`, `5m rate`, `Top 12 of 40`. A widget showing twelve of forty runtimes
   titled "Runtime latency" is a claim about forty runtimes.

> **Violation.** `config/metrics.json` `fleet/vram` computes a percentage and
> labels the gauge `VRAM used`. The `gauge` kind has no `unit` field, so the
> label is the only place a unit can live; a dial reading 78 labelled "VRAM
> used" is a number with no magnitude. `fleet/gpu-util` has the same shape.
> Generated candidates label these `VRAM used (%)` and `GPU utilization (%)`.

## The emitter constrains what may be proposed

`canvas-metrics::widget_within` produces exactly five widget kinds. Every other
tag falls through its `match` to `None`, which means no snapshot is ever sent,
which means the widget stays `Loading` forever.

**A survey may only emit a candidate whose `source.kind` is one of `stat`,
`gauge`, `ranked_bar`, `time_series`, `heatmap`.** This list is a constant
shared with `canvas-metrics`, not a literal retyped in the survey.

The emitter also caps every collection, and the caps are lower than
`WidgetSpec::validate` allows, so the emitter is the limit that binds:

| `source.kind` | Emitter cap | What the survey must do |
|---|---|---|
| `stat` | uses `samples[0]` | query must return exactly **one** series; verification asserts it |
| `gauge` | uses `samples[0]`, clamps to `0..100` | one series, already scaled into `0..100` |
| `ranked_bar` | 12 bars, sorted, truncated | over 12 series, title says `Top 12 of N` |
| `time_series` | 8 series, 180 points, thinned to fit | over 8 series, title says `8 of N` |
| `heatmap` | 12 rows, 48 columns | over 12 series, title says `12 of N` |

The `stat` rule is the sharp one. `widget_within` takes the first sample of
whatever came back, so a `stat` over a multi-series query renders one arbitrary
series as though it were the whole answer, with nothing on the face of the
widget to say so. That is not a `Loading` bug — it is a widget that lies. The
survey rejects such a candidate at verification rather than emitting it.

Snapshots are additionally fitted to `MAX_DATAGRAM - ENVELOPE_OVERHEAD`
(2048 − 420 bytes; macOS caps an `AF_UNIX` datagram at
`net.local.dgram.maxdgram` well below Canvas's own 64KB ceiling). Thinning
handles this at emit time, so the survey does not need to model byte budgets —
but a candidate whose label set is so wide that even 2 points per series will
not fit is not a candidate.

## The candidate document

One file per surveyed subject, `config/survey/<subject>.candidates.json`. The
examples below are shaped, not observed: the metric names are real, the counts
and values illustrate the format, and no survey has run yet.

```json
{
  "schema_version": 1,
  "kind": "canvas.survey.candidates.v1",
  "survey": {
    "surveyed_at": "2026-09-10T06:12:04Z",
    "prometheus": "https://prom.fpl.dev",
    "selector": "{job=~\"bifrost|palisade|invoker|fpl-auth\"}",
    "metric_names": 214,
    "accounted": 214
  },
  "subject": {
    "id": "fleet",
    "name": "FPL serving fleet",
    "does": "Bifrost routes model requests to Yggdrasil GPU runtimes; Palisade fronts every service as the authenticating gateway; Invoker runs on-demand functions; fpl-auth issues the sessions all of it checks.",
    "symptom": "When it breaks, the symptom a person sees is a request that hangs or 502s."
  },
  "candidates": [],
  "unverified": [],
  "unclassified": [],
  "excluded": []
}
```

`subject` is the one block that is written by a person and not derived.
Nothing in a metric inventory says what a service is for or what its failure
looks like from outside, and a rule that guessed would be an opinion wearing a
derivation's clothes. One or two sentences, supplied when the survey is run,
carried in the file so the selector receives it with the candidates.

### A candidate

```json
{
  "id": "fleet-gpu-memory",
  "source_id": "fleet/gpu-memory",
  "source": {
    "kind": "gauge",
    "query": "100 * sum(bifrost_yggdrasil_accelerator_memory_used_bytes) / sum(bifrost_yggdrasil_accelerator_memory_bytes)",
    "label": "GPU memory in use (%)"
  },
  "spec": {
    "schema_version": 1,
    "id": "fleet-gpu-memory",
    "kind": "gauge",
    "value": 61.4,
    "label": "GPU memory in use (%)",
    "data": {
      "state": "ready",
      "source": "prometheus:fleet/gpu-memory",
      "observed_at": "2026-09-10T06:12:04Z",
      "window": "now",
      "provenance": "sample"
    }
  },
  "evidence": {
    "metrics": [
      "bifrost_yggdrasil_accelerator_memory_used_bytes",
      "bifrost_yggdrasil_accelerator_memory_bytes"
    ],
    "type": "gauge",
    "help": "Accelerator memory currently allocated, in bytes",
    "prometheus_unit": "bytes",
    "series": 8,
    "label_cardinality": { "accelerator": 8, "node": 2 },
    "family": "bifrost_yggdrasil_accelerator_memory"
  },
  "rule": "family/used-over-total -> one utilisation gauge",
  "verified": {
    "at": "2026-09-10T06:12:04Z",
    "series_returned": 1,
    "sample_value": 61.4
  }
}
```

| Field | Meaning |
|---|---|
| `id` | Stable within a survey. Matches `spec.id`, so a selection can name either. |
| `source_id` | The `owner/name` key this becomes in `config/metrics.json`. `canvas-metrics::stream_identity` splits it on `/`, so it must contain exactly one. |
| `source` | **Verbatim a `SourceSpec`.** Nothing added, nothing renamed. |
| `spec` | **Verbatim a `WidgetSpec`.** Parses with `WidgetSpec::from_json` and passes `validate()`. |
| `evidence` | What was observed. Facts, not conclusions. |
| `rule` | Which rule fired, named. One line, human-readable, stable across runs. |
| `verified` | The result of executing `source.query`. |

The two payload subtrees are byte-identical to what they become. Accepting a
candidate is copying `source` under `sources[source_id]` in
`config/metrics.json` and copying `spec` into a view — a move, not a
translation. The survey's reasoning sits **beside** them, never inside them, so
there is nothing to strip on the way in and nothing to invent on the way out.

### Payload values are sampled, not zeroed

A candidate's `spec` carries the values the verification query actually
returned, with `provenance: "sample"` and an `observed_at`. `Provenance::Sample`
is defined as "representative values shipped as documentation of the payload
shape", which is exactly what these are, and it renders with a persistent
`SAMPLE` badge so a candidate previewed on a screen can never be mistaken for
a live one.

It must **not** be `{"value": 0, "data": {"state": "loading"}}`. All 24 widgets
across `config/views/*.json` are written that way today — a zero, or an empty
list, under `state: "loading"` — and a zero is worse than blank because it is a
claim. Rule 3 of `docs/widget-quality.md` applies to generated content: absent
data says why, in the widget's own words.

### The four accounting lists

Every metric name returned by `/api/v1/label/__name__/values` appears **exactly
once** across: some candidate's `evidence.metrics`, `unverified`,
`unclassified`, or `excluded`. `survey.accounted` equals
`survey.metric_names`, and a test asserts it.

This is the honesty requirement made mechanical. Silent omission is how a
dashboard ends up quietly missing the metric that mattered.

Accounting is over **names**, not over metrics-as-a-person-thinks-of-them. A
histogram contributes three names — `m_bucket`, `m_sum`, `m_count` — and the
histogram rule claims all three into one candidate's `evidence.metrics`, which
is also why the `_sum`/`_count` family rule does not fire on them a second time.

| List | Entry means | Fields |
|---|---|---|
| `unverified` | A rule fired and produced a query; executing it returned no series | `metrics`, `rule`, `query`, `reason` |
| `unclassified` | No rule fired | `metric`, `type`, `help`, `series`, `reason` |
| `excluded` | A rule fired that says "not a widget" | `metric`, `rule`, `reason` |

`unverified` carries the failing query verbatim, because the most common cause
is a label the rule assumed and the metric does not have, and that is a
thirty-second fix for a person holding the query text.

### Verification is not optional

Every `source.query` is executed against Prometheus before the candidate is
written. A query that returns nothing does not become a candidate. This is the
single rule that makes the permanent-`Loading` failure structurally impossible
to generate: an emitted candidate is one that was observed to return data.

Verification also asserts the emitter's preconditions, so `series_returned` is
a checked value and not a note: exactly 1 for `stat` and `gauge`, at least 1
otherwise.

### Over-generation is deliberate

A survey emits more candidates than any dashboard should hold — including
several from one metric where several shapes are defensible (a `stat` of
`sum(bifrost_backend_up)` and a `ranked_bar` of `bifrost_backend_up` are both
legitimate readings of the same gauge). Narrowing is cheap and it is the
selector's job. Noticing an absent metric after the fact is expensive.

## The selection document

`config/survey/<subject>.selection.json`. Produced by stage 5, consumed by
stages 6 and 7.

```json
{
  "schema_version": 1,
  "kind": "canvas.survey.selection.v1",
  "from": { "subject": "fleet", "surveyed_at": "2026-09-10T06:12:04Z" },
  "view": {
    "id": "fleet-runtime",
    "question": "Is the serving fleet up, failing, or slow right now?"
  },
  "selected": [
    { "rank": 1, "candidate_id": "fleet-backends-reachable", "title": "Model backends reachable", "because": "..." }
  ],
  "rejected": [
    { "candidate_id": "fleet-sessions", "because": "..." }
  ],
  "missing": [
    { "want": "...", "because": "..." }
  ]
}
```

| Field | Meaning |
|---|---|
| `view.question` | The one question this view answers. A view that answers two is two views. |
| `selected[].rank` | Incident-reading order, 1 first. Not layout. |
| `selected[].title` | May override the candidate's generated title. Subject to the title rules above. |
| `selected[].because` | Why this earned a slot on a wall. |
| `rejected[].because` | Why a candidate that exists did not earn one. |
| `missing[]` | A view the selector wanted and no candidate provides. |

**There is no field in this document in which a query could be written.** That
is deliberate: a fabricated PromQL expression that returns nothing is the exact
failure the pipeline exists to remove, and the cheapest way to prevent it is to
give the fabricator nowhere to put it. A selector that wants a view no
candidate carries writes it in `missing` in English, and a person adds a rule
or a recording rule.

`rejected` is not optional padding. A selection that lists ten choices and no
rejections has not been reviewed, it has been produced.

## Layout is derived

Stage 6 receives the ordered `selected` list, nothing else about geometry.

1. Reading order follows `rank`: first widget top-left, then across, then down.
2. Each widget is placed at its `size_hint` **comfortable** size for its kind.
3. Slack goes to kinds whose comfortable height exceeds their minimum by a
   real margin — `time_series`, `heatmap`, `histogram`, the tabular kinds. A
   `stat`, `gauge` or `clock` gains nothing from height, and room given to one
   is room a chart could have used.
4. `preferred_aspect` is honoured where there is slack to spend, and dropped
   before a minimum is violated. It is a preference, never a lock.
5. Display count comes from the platform, re-read on reconfiguration
   (`dock-blw`), not from a constant.
6. If the selection cannot fit at **minimum** size, layout **fails loudly** and
   names the widget it could not place. It does not shrink below minimum. A
   widget squashed until its label truncates is already unreadable, and
   silently producing one moves the cost to whoever glances at the wall.

Rule 6 is why the selection prompt asks for six to ten and not "as many as are
good".

## Installation

| Artifact | Lands in |
|---|---|
| each `selected` candidate's `source` | `config/metrics.json` under `sources["<source_id>"]` |
| each `selected` candidate's `spec` | `config/views/<view.id>.json` under `widgets` |
| geometry | the `rect` beside each spec, written by stage 6 |

Installation is a merge, not an overwrite: an existing hand-written source with
the same key is left alone and reported, because the hand-written one may
encode something the survey cannot see.

## What is checked where

| Rule | Enforcement |
|---|---|
| Every metric accounted for exactly once | survey test: `accounted == metric_names` |
| Emitted query returns ≥1 series | executed before write; failure demotes to `unverified` |
| `stat`/`gauge` query returns exactly 1 series | verification asserts `series_returned == 1` |
| `source.kind` is one the emitter produces | constant shared with `canvas-metrics` |
| `spec.data.source == "prometheus:" + source_id` | survey test over every candidate |
| `spec` is a valid widget | `WidgetSpec::from_json` + `validate()` per candidate |
| Sampled payload, never a zero placeholder | survey test: `provenance == "sample"`, `state == "ready"` |
| Unit present exactly once | survey test, per kind |
| Truncation disclosed in the title | survey test where `series` exceeds the emitter cap |
| Selection names only existing candidate ids | selection linter |
| Selection invents no PromQL | structural: the schema has no query field |
| Selection is six to ten widgets, or every verified candidate when fewer exist | selection linter |
| Layout never places below minimum | layout fails and names the widget |
| Titles state what is measured | review, at the selection boundary |
| The six-to-ten are the right six-to-ten | review — this is the taste, and it is the point |

Everything above the last two lines is mechanical. The last two need a person,
which is why they are the two worth spending review time on.

## Runbook

The manual version, for when the automation breaks or before it exists:

1. `curl` `/api/v1/metadata` for the job. Read the `type` and `help` for each
   metric.
2. Group by longest common prefix. Look for `_used_bytes`/`_bytes`,
   `_total`/`_errors_total`, `_sum`/`_count` pairs and treat each pair as one
   widget.
3. For each remaining metric, apply the rule table. Counters get `rate()`.
4. Run every query you wrote. Delete the ones that return nothing and write
   down why you think they were empty.
5. Write down what the service is and what its failure looks like from outside.
6. Pick six to ten. Order them the way you would check them at 3am. Write one
   sentence for each about why it is there, and one for each of the ones you
   dropped.
7. Title each with its unit.
8. Place them in reading order at the sizes `size_hint` gives.

Stage 5 is step 6. It is the only step where two competent people produce
different output, and that is the test for whether a stage belongs to a model.

## Not yet built

- `fpl-desktop-bridge/src/metric_reads.rs` allows three routes (`query`,
  `query_range`, `/-/ready`); the survey needs the three metadata routes added
  to the same allowlist, enumerated the same way.
- `canvas-metrics` emits five widget kinds. `node_summary`, `histogram`,
  `state_timeline` and `event_log` have no emitter, so the survey cannot
  propose them however well they fit — `bifrost_backend_up` per backend wants a
  status list and cannot have one. Widening the emitter widens the rule table.
- `ranked_bar` truncates to 12 bars inside `canvas-metrics` with no disclosure
  on the widget's face. Until it discloses, the disclosure lives in the
  generated title, which is a workaround and should be named as one.
