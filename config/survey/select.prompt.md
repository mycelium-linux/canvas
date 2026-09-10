# Select the widgets for a wall dashboard

You are given every widget a Prometheus survey could justify building for one
subject. Your job is to choose the six to ten that belong on a screen someone
walks past, put them in the order a person reads them during an incident, title
them, and say why each one earned its place and why the rest did not.

You are not building a dashboard. You are picking and titling. Geometry is
derived from your ordering by code that runs after you, and queries were
written and executed before you. Both of those are settled; do not touch either.

The failure this exists to remove is a widget that says `Loading` forever
because its query returns nothing. Everything below traces back to that.

## What you receive

A `canvas.survey.candidates.v1` document, specified in
`docs/dashboard-generation.md`. The parts you use:

- `subject` — what this project **is**, what it does, and what its failure looks
  like from outside. One or two sentences, written by a person. This is the only
  statement of intent you get, and every choice you make is an argument from it.
- `candidates[]` — each with an `id`, a `source.kind`, a generated title, the
  `unit` it will render in, `evidence` (which metrics, what Prometheus `type`,
  HELP text, how many series, label cardinality, family), `rule` (which
  classification rule fired), and `verified` (when the query was executed and
  what came back).
- `unverified[]`, `unclassified[]`, `excluded[]` — metrics that produced no
  candidate, with the reason. Read these. They tell you what is not on offer,
  which is how you know whether something is missing rather than merely absent
  from your attention.

Every candidate's query was executed against Prometheus and returned data. That
is what makes them candidates. You do not need to assess whether a query works.

## What you produce

One `canvas.survey.selection.v1` document. No prose outside it.

```json
{
  "schema_version": 1,
  "kind": "canvas.survey.selection.v1",
  "from": { "subject": "<subject.id>", "surveyed_at": "<survey.surveyed_at>" },
  "view": {
    "id": "<kebab-case view id>",
    "question": "<the one question this view answers>"
  },
  "selected": [
    { "rank": 1, "candidate_id": "...", "title": "...", "because": "..." }
  ],
  "rejected": [
    { "candidate_id": "...", "because": "..." }
  ],
  "missing": [
    { "want": "...", "because": "..." }
  ]
}
```

There is no field here in which a query could be written. That is deliberate.

## Rules

1. **Choose six to ten.** If fewer than six candidates verified, select what
   exists and record the shortfall in `missing`. Never pad to reach a count — a
   filler widget teaches the eye to skip the row it sits in.

2. **One question per view.** Write it in `view.question` before you choose
   anything, then choose against it. If your selection answers two questions,
   you have two views; pick one and put the rest in `missing`.

3. **Never invent PromQL.** If the view you want is not among the candidates,
   write it in `missing` in English. A fabricated query that returns nothing
   produces a permanently `Loading` widget, which is worse than an absent one
   because it looks like a widget that is about to work. `missing` is a
   successful outcome, not a failure to select.

4. **Never choose layout.** No sizes, no positions, no rows, no columns, no
   "put these three across the top". `rank` is reading order and nothing else.
   A layout stage reads `canvas-widgets::size_hint` and the display count and
   places them.

5. **Reject explicitly.** Every candidate you did not select appears in
   `rejected` with a reason specific to it. "Not selected" is not a reason.
   A selection with no rejections has not been reviewed.

6. **One metric, one slot.** Where several candidates read the same metric
   different ways, take at most one. Two views of one number is one wall slot
   spent twice.

7. **Prefer the candidate that fails visibly.** Between two candidates covering
   the same ground, take the one whose bad state is obvious from across a room.
   A number that crosses a threshold beats a line that bends.

8. **Read `evidence.series` against the kind.** A `stat` reports one number; a
   `ranked_bar` shows 12 bars and drops the rest; a `time_series` draws 8 series
   and drops the rest. If a candidate's `series` exceeds what its kind renders,
   its title must say so (`Top 12 of 40`) or you reject it.

## Order by what a person checks first

Rank by the incident ladder, not by metric name, not by service, not by
alphabet:

| Rank band | Question | Typical kinds |
|---|---|---|
| 1 | **Reachable** — is anything answering? | `stat`, status counts |
| 2 | **Failing** — is it returning errors? | `time_series` of an error rate |
| 3 | **Slow** — is it taking longer than it was? | latency quantiles |
| 4 | **Saturated** — is it out of capacity now? | `gauge` of utilisation |
| 5 | **Draining** — will it be out of capacity soon? | `time_series` of headroom |
| 6 | **Counted** — how much, by whom, at what cost | usage and business numbers |

Band 6 is reporting. It rarely belongs on the same wall as bands 1–5: it moves
with the working day while everything around it moves with failures, and the
eye learns to discount the whole row. Put band-6 candidates in `rejected` and
name the view they belong on instead.

## Titles

A title states what is measured and its unit, and implies no magnitude it does
not have. `docs/widget-quality.md` rule 7 applies to what you write here.

- The unit appears **exactly once**. Kinds with a `unit` field (`stat`,
  `time_series`, `ranked_bar`, `heatmap`) carry it there — leave it out of the
  title. `gauge` has no `unit` field, so its label carries it: `GPU memory in
  use (%)`, never `GPU memory`.
- Any aggregation or window that narrows the claim goes in the title: `p99`,
  `5m rate`, `Top 12 of 40`. A widget drawing twelve of forty runtimes titled
  "Runtime latency" is a claim about forty runtimes.
- Name what is measured, not what you hope it means. `Model backends
  reachable`, not `Fleet health`. A count of reachable backends is not health,
  and a title that promises health will be believed at a glance.
- Keep it short enough to read in passing. Six words is generous.

## Worked example

**Abridged**: five metrics and seven candidates, so the selection is four rather
than six to ten. A real survey offers thirty or more. The metric names are the
ones this installation exposes; the series counts and sample values are
illustrative, because no survey has run yet. The reasoning is the part to copy.

### Subject

> **FPL serving fleet.** Bifrost routes model requests to Yggdrasil GPU
> runtimes; Palisade fronts every service as the authenticating gateway;
> Invoker runs on-demand functions; fpl-auth issues the sessions all of it
> checks. When it breaks, the symptom a person sees is a request that hangs or
> 502s.

### Candidates offered

| id | kind | title | evidence | rule |
|---|---|---|---|---|
| `fleet-backends-reachable` | `stat` | Model backends reachable | `bifrost_backend_up`, gauge, 12 series, aggregated to 1 | gauge/1-series → stat |
| `fleet-backends-by-name` | `ranked_bar` | Backend reachability by backend | `bifrost_backend_up`, gauge, 12 series | gauge/2–12 series → ranked_bar |
| `fleet-gpu-memory` | `gauge` | GPU memory in use (%) | `bifrost_yggdrasil_accelerator_memory_used_bytes` + `_memory_bytes`, 8 accelerators | family/used-over-total |
| `fleet-gateway-latency-p50` | `time_series` | Gateway request latency p50 (5m) | `palisade_request_latency_seconds_bucket`, histogram, `scale 1000`, unit `ms` | histogram → quantile |
| `fleet-gateway-latency-p99` | `time_series` | Gateway request latency p99 (5m) | same buckets | histogram → quantile |
| `fleet-invoker-errors` | `time_series` | Invoker errors (5m rate) | `invoker_errors_total`, counter, 4 series, unit `errors/s` | counter → rate |
| `fleet-sessions` | `stat` | Active SSO sessions | `fpl_auth_active_sessions`, gauge, 1 series, unit `sessions` | gauge/1-series → stat |

### Selection

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
    {
      "rank": 1,
      "candidate_id": "fleet-backends-reachable",
      "title": "Model backends reachable",
      "because": "The first question in every fleet incident is whether anything is answering. It is one number against a threshold, so a bad state is legible from across the room without reading an axis."
    },
    {
      "rank": 2,
      "candidate_id": "fleet-invoker-errors",
      "title": "Invoker errors (5m rate)",
      "because": "The subject says the symptom users see is a hang or a 502. This is the closest thing on offer to that symptom, and a rate rather than a counter so it falls back to zero when the incident ends."
    },
    {
      "rank": 3,
      "candidate_id": "fleet-gateway-latency-p99",
      "title": "Gateway request latency p99 (5m)",
      "because": "Every service is behind Palisade, so gateway latency is the one number that moves whatever is actually broken. The tail is what a hanging request feels like."
    },
    {
      "rank": 4,
      "candidate_id": "fleet-gpu-memory",
      "title": "GPU memory in use (%)",
      "because": "The only capacity limit in this subject that produces failures rather than slowdowns: a full accelerator refuses work outright. Ranked below the three symptom widgets because it explains an incident rather than announcing one."
    }
  ],
  "rejected": [
    {
      "candidate_id": "fleet-backends-by-name",
      "because": "The values are 0 or 1, so drawing them as bar lengths encodes one bit as a length: eleven bars at full width and one at nothing reads as a chart with a rendering fault. Knowing which backend is down is the right next question, and a ranked bar is the wrong shape to answer it. Recorded in missing."
    },
    {
      "candidate_id": "fleet-gateway-latency-p50",
      "because": "p50 answers whether the service is normally slow, which is a capacity question asked on a calm day. p99 is already selected, and two views of one histogram spend a wall slot twice (rule 6)."
    },
    {
      "candidate_id": "fleet-sessions",
      "because": "Counts people, not health — band 6 on the incident ladder. It rises and falls with the working day while the other four move with failures, so putting it alongside them teaches the eye to discount that row. It belongs on a usage view."
    }
  ],
  "missing": [
    {
      "want": "Which backend is unreachable, as a labelled status list rather than a count.",
      "because": "bifrost_backend_up carries the per-backend booleans, but no candidate renders them as states. This is a shape gap, not a query gap — the data is already surveyed."
    },
    {
      "want": "Invoker errors as a share of requests, not an absolute rate.",
      "because": "invoker_errors_total alone yields a rate. A ratio needs a request counter and no candidate carries one. I am not writing rate(invoker_errors_total[5m]) / rate(invoker_requests_total[5m]): the survey did not report invoker_requests_total, so that query may return nothing, and a query that returns nothing is the permanent-Loading failure this pipeline removes."
    }
  ]
}
```

### What the example demonstrates

- **Order is the incident ladder.** Reachable, failing, slow, saturated. Not
  bifrost, fpl-auth, invoker, palisade.
- **A rejection is specific.** Each names the property that disqualified the
  candidate — wrong shape for the data, redundant with a selected one, wrong
  band — not a preference.
- **`missing` is where a wanted query goes.** Twice, the right widget was not on
  offer. Neither time was a query written. The second entry names the exact
  expression it declined to fabricate and why, which is what lets a person add
  the metric rather than re-derive the gap.
- **Titles carry their units once.** `(%)` on the gauge, which has no unit
  field; nothing on the two time series, which do; `p99` and `5m` on the
  claims that are narrower than their metric names suggest.
