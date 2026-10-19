# First-party coding and messaging pass

October 19, 2026. Baseline: Canvas `ae9fdc5`, isolated branch
`feat/shared-workspace-egui`. This is the first read-only adapter slice, not a
claim that interactive coding or messaging views are deployed.

## Follow the Panopticon boundary

The reference is the implemented [Panopticon source/presenter](panopticon-wall.md),
not an embedded copy of each service's web app:

1. An owner-local connector reads an already authorized owner API.
2. A bounded, credential-free observation travels over the existing Unibus.
3. A recipient pins source identity and scope, rejects expired/replayed data,
   and composes the observation locally.
4. Existing Canvas control delivers transient state to an enrolled surface.
   Canvas selection stays local. Installation does not activate a workspace.

Unibus owns registration, grants, framing, TTL and routing. These adapters do
not implement a second bus or change its opaque-body policy. Domain meaning
lives in `canvas-widget-host::first_party`. Full snapshots recover a late
subscriber on the next observation; there is no claim of durable replay,
historical delivery, or exactly-once command execution.

## Two workspaces

### Verified ohm production owner

Newer ohmOS source `6565caa` points to the existing first-party Rust
`ohm/dumb-bridge` (`a4a2583`). The deployed Studio Iroh bridge responds at
loopback port 8790; `ohm.fpl.dev/messages/` reaches it through the provisioning
app's passkey-protected proxy. An authenticated read on October 19 returned
four non-demo accounts (Signal, SMS, Telegram, WhatsApp), 73 conversations and
zero source errors. No threads were fetched and no read state was changed.

Use this owner, not the legacy emulator daemon, for the production-facing
adapter. Its Snapshot already contains explicit account IDs, per-account demo
flags, conversations, errors and capabilities. The Canvas projection enrolls
exact account IDs, retains account/conversation identity and unread counts,
omits previews/recipient data, and preserves partial source failures. Mixed
demo/live selections are marked sample. Unknown/legacy adapter account prefixes
cannot silently become live. Missing selected accounts mean partial/unavailable,
never a reassuring empty inbox.

The bridge already owns `messaging.request.v1` and `messaging.reply.v1` over
Unibus. `canvas-first-party messaging CONFIG CYCLES` uses exactly its `snapshot`
read operation, pins the provider, destination and request correlation, and
emits bounded Canvas observations. There is no send/approve command in this
adapter. The bridge's existing caller-role map must enroll the Canvas producer
as a reader in addition to the hub's transport grants.

The current bridge LaunchAgent has no `UNIBUS_CONFIG` entry, so code support is
not evidence of deployment enrollment. That enrollment/readback is tracked in
`ohmos-canvas-owner-observations`; pairing keys, accounts and send policy remain
untouched. The HTTP `observe` mode and stdin `project` mode are diagnostics.
`publish` refuses `ohm_bridge`: do not invent another messaging RPC or republish
the existing provider through a parallel domain route.

**Coding:** Alfalfa is project purpose, resources, context and repository
attention. Edgerunner is owner-authorized coding sessions and their execution
state. Display these together under the actual Alfalfa project ID. Do not join
by title, repository basename, or an agent-controlled label. The initial
connector explicitly enrolls session IDs to a project; this is presentation
association, never authorization. Edgerunner must first authorize the session
read itself. A session absent from the authorized response is not displayed.

**Messaging:** ohmOS and its bridge own accounts/platform clients, threads, unread state,
delivery and replies. The first view projects conversation titles, platform and
unread counts. Message text, recipient IDs and attachment references are not
published. Conversation identity is `(source instance, authorized store scope,
owner account ID, owner conversation ID)`. The legacy emulator DTO lacks an
account identifier and remains a separate sample-only diagnostic. Beeper is
deferred in this first-party pass.

The summary binder uses the existing status-list primitive for adapter
verification. It does not replace the ongoing dense egui inbox work, implement
a thread reader, or add selection/navigation behavior to a static list.

The recipient-local `first_party::coding` function combines context and sessions
under one exact project ID, preserving each source's state and marking stale
sources independently. A manual bounded composition is available with
`canvas-first-party coding PROJECT < OBSERVATIONS.json`; the input contains
`context` and `sessions` projections (either may be null). Pass observations
already admitted by the configured bus receiver. This diagnostic validates the
aggregate, not source authorization, and performs no owner fetch or UI install.

## Implemented owner reads

| Owner | Read | Preserved meaning |
| --- | --- | --- |
| Alfalfa | `GET /api/v1/projects/{scope}/view` | Requires `alfalfa.manifest-view/1` and exact project ID; purpose/status stay owner-authored. |
| Edgerunner | `GET /v1/owned-sessions` | Requires schema version 1; only explicitly enrolled session IDs from this authorized listing appear. Missing session update times stay unknown. |
| ohm bridge | Unibus `snapshot`, or diagnostic `GET /api/snapshot` | Explicit account IDs, demo flags, conversation IDs, unread counts, timestamps and source errors. |
| Legacy ohmOS emulator | `GET /inbox` | Sample-only diagnostic; no production account/provenance claim. |

The reviewed source contracts are Alfalfa `manifest_view.rs`, Edgerunner
`session_readiness.rs`, and ohmOS `wire.rs`. The local checkouts are not proof
of their live deployments. In particular, the legacy ohmOS HTTP seam lacks
the complete internal reply/read-receipt/history interface. Its send endpoint
can report queued without exposing final delivery. No send action is offered
by this slice. Alfalfa attention remains its existing
`alfalfa.project-attention/1` contract; do not recalculate Git/PR health in Canvas.

Rechecked ohmOS upstream `6565caa` (October 10) without changing its local
working tree: `ohm-msgd` still seeds `Inbox::sample()` without its Telegram
feature and can run scripted sources. Its legacy inbox DTO has no provenance
field. Consequently this adapter always marks that legacy ohmOS projection
`sample`, with an explicit unverified-source caption; the receiver rejects a
legacy ohmOS projection claiming `live`. This is a conservative diagnostic,
not a verified live messaging inbox. The production bridge interface above
already provides the owner boundary to use instead. A successful HTTP read is not
evidence that conversations came from real accounts. The public `ohm.fpl.dev`
origin also serves provisioning/update concerns; its root is not proof of a
deployed messaging endpoint.

## Bounds and freshness

At most 4 MiB is read from an owner, with a six-second HTTP deadline and no
redirects. Remote origins require HTTPS; plaintext is loopback-only. Bearer
values can be read from an existing private file, never placed in a manifest,
bus body or command argument. This is not an OAuth acquisition mechanism.

An observation has at most 64 displayed rows, 4096 input rows, an explicit
omitted count, and a 64 KiB serialized projection. Owner IDs remain opaque;
unknown/private owner fields are discarded. Duplicate or malformed rows reject
the observation rather than becoming an empty inbox. A successful empty read
and a disconnected source are distinct. Owner failure publishes a disconnected
sample with no renewed old rows.

`read_at_unix_ms` means an HTTP read completed, not that an underlying message
or session changed. ohmOS `last_ts` is preserved separately in milliseconds.
Recipients reject observations older than 20 seconds, over five seconds in the
future, or at/below their per-source `(read time, sequence)` cursor. Restarted
producers recover through a newer read timestamp. Receivers must also expire
their displayed state locally after source/transport loss; bus TTL alone cannot
expire an already rendered view. Use a 5-second widget refresh policy on the
receiving workspace and verify the endpoint's actual stale behavior.

## Manual runbook

Build/check the small standalone host crate, not the compositor workspace:

```sh
cargo check --manifest-path crates/canvas-widget-host/Cargo.toml --features owner-http --lib --bin canvas-first-party
cargo test --manifest-path crates/canvas-widget-host/Cargo.toml --features owner-http --lib --bin canvas-first-party --bin canvas-widget-host
lua config/binders/tests/first-party-v1-test.lua
```

HTTP dependencies are behind `owner-http`; ordinary local composition and the
projection library do not pull an HTTP client into the display host.

Create an operator-private configuration, for example:

```json
{
  "owner": "alfalfa",
  "source": "projects",
  "scope": "alfalfa",
  "base_url": "http://127.0.0.1:8791"
}
```

For Edgerunner use `owner: "edgerunner"`, its existing authorized origin,
`bearer_file` if required, and `session_ids: ["an-existing-owned-session"]`.
For the production bridge use `owner: "ohm_bridge"`, its explicitly selected
private origin and `account_ids` from the owner's authenticated snapshot.
The verified Iroh account IDs are `iroh:signal`, `iroh:sms`, `iroh:telegram`,
and `iroh:whatsapp`; these are instance-specific enrollment evidence, not defaults
for arbitrary users. `owner: "ohmos"` is only the sample legacy diagnostic.
The program derives the read path; input data cannot redirect its requests.

For a captured owner response, use
`canvas-first-party project private-owner.json OWNER_READ_AT_MS < snapshot.json`.
Supply its actual read time; reading a cached file must not renew its freshness.
The binder independently marks observations stale after 20 seconds and rejects
future timestamps. Keep snapshots/projections private; source snapshots can
contain previews even though the Canvas projection discards them.

```sh
canvas-first-party observe private-owner.json
# Pipe the bounded observation to the existing composer:
canvas-first-party observe private-owner.json | canvas-widget-host config/binders/first-party-v1.lua
```

For finite bus validation add `bus` using the existing Unibus ClientConfig,
schema version 2, a private token-file reference and a loopback authenticated
tunnel. The source and recipient need independent credentials. The recipient
also pins `source_producer` and the same owner/source/scope:

```sh
canvas-first-party receive private-recipient.json 3
canvas-first-party publish private-source.json 3
```

For ohm, use `canvas-first-party messaging private-ohm-reader.json 3` instead.
It sends `messaging.request.v1` directly to the pinned bridge producer and
accepts only matching `messaging.reply.v1` responses addressed to this reader.
Use the bridge's existing caller map and request/reply routes, not the generic
projection publication route below. Its role should be `reader`.

The generic coding projection enrollment requires an explicit `canvas.first-party.snapshot.v1` route to the
recipient capability, scoped publish kinds/producers and private zones. The
program uses destination `first-party`. This is a proposed new route, not an
assertion that the current hub accepts it. Preserve existing routes/grants;
do not reuse Panopticon's publication credentials or its public display zone.
The receiver emits newline-delimited observations for the existing binder and
`canvas-widget-host stream-tv` path, not provider data fetching on the TV.

Before installation, read the current workspace/catalog revision, declare only
the intended surfaces, and preserve other projects and selection. Verify
independent consumer readback and source timestamps. Do not treat a source
publication log as proof of receipt. No services, hub grants, workspaces or
APKs are installed by the commands above.

## Remaining product acceptance

The durable issue `dock-workspace-first-party` tracks live owner enrollment,
scoped hub routing and readback, recipient-local stale expiry, coding project
aggregation, selected session context, messaging thread selection, and the
interactive dense component integration. Owner commands and authenticated
thread access need their actual contracts verified before adding UI actions.
The existing Panopticon deployment remains untouched.

## Verified first slice

On October 19, the exact source and lockfile were copied to an isolated Neo
temporary checkout using its current sibling Unibus SDK. `cargo check`, all
27 selected Rust tests, `cargo clippy ... -- -D warnings`, artifact builds,
changed-file rustfmt checks, and the standalone Lua contract test passed.
Tests include the actual Luau host, empty arrays, UTF-8 truncation, per-source
expiry, account isolation, demo provenance, partial failures, HTTP read shaping,
source/destination pinning and messaging request correlation. This is host-side
macOS verification; it is not Linux/Fire TV/WebGPU rendering qualification.

The compiled adapter consumed a fresh authenticated Studio bridge snapshot and
composed it through the actual bounded Canvas Luau host: `ready`, provenance
`live`, 73 conversations, 64 displayed rows, 9 explicitly omitted, zero source
errors, across the four enrolled Iroh accounts. A structural check found no
`preview`, `recipient`, `text` or `remote_id` fields in the Canvas projection.
These are data/composition receipts, not a screenshot or end-to-end bus receipt.
No account connection, send, read receipt, live hub grant, LaunchAgent, APK or
workspace selection was changed. Parent acceptance and live enrollment remain open.
