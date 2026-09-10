# Federated context — the decision

*This resolves the four architecture proposals for cross-machine context. Every claim below was verified by reading the file cited. No cargo was run. Where a proposal or an attack asserted something I could not confirm, it is marked as a gap rather than repeated.*

---

## 1. The recommendation

**Adopt proposal 4's scope model and reject all three of its concrete service claims.** A scope is a closed, wildcard-free `ScopeRef` enum with no containment relation; every context payload is assembled from a scope key rather than filtered after assembly; service boundaries are drawn where two pieces of state stop sharing one authorization decision. On top of that: **disclosure is two predicates, not one** — a *principal* predicate evaluated at assembly, and a *room-at-emission* predicate evaluated immediately before every emission and re-evaluated on every re-emission — because Sound and Canvas do not share an audience algebra and forcing them under one authority is what killed proposal 1. **fpl-auth answers identity and verbs only; the service that owns a row answers objects**, because `can_use_scope` takes no context argument (`fpl-auth/src/main.rs:11069-11074`), `guest_grants` is keyed on email alone (`main.rs:5850-5855`), and there is no `project_memberships` table anywhere in fpl-auth (`grep`: zero hits). **No new coordinator service, no service merge, no replication**: the two durable stores share no key space today, so what is common between them is a library (`fpl-scope`, one envelope crate), not a network hop. Remote and mobile clients attach through a **Perimeter that holds no durable state and terminates exactly one `ScopeRef` per connection** — built on a user-authenticated path, *not* on `/credentials/desktop-bridge/resolve`, which authenticates a static service secret and takes the user identity as an unauthenticated request field (`fpl-auth/src/main.rs:4008-4053`, `9017-9026`).

---

## 2. Why — and which leak killed which proposal

A proposal with a structural leak is dead. Not "needs work" — dead, because the leak is in the sentence the design exists to justify.

### Proposal 1 is dead: two audience algebras

The premise is *"the thing that decides who may see something must be the same thing that assembles what they see."* It is false in this codebase, and the falseness is load-bearing.

Sound's private-disclosure gate is a predicate over **the room at a moment**:

```rust
Audience::Private(person) => {
    p.privacy.tact == DisclosureTact::Direct
        && p.privacy.room_is_private
        && !p.privacy.others_present
        && !p.observed_identities.is_empty()
        && p.observed_identities.iter().all(|identity| {
            identity.person_id.as_ref() == Some(person) && confident(identity.confidence)
        })
}
```
— `jarvis-subconscious/src/background.rs:872-885`, with its twin at `attention.rs:263-275`.

The proposed primitive is `authorize(principal, scope, capability) -> Lease` and `assemble(&lease, ...)`, which carries no zone and no timestamp. A patch stream has no "immediately before playback" instant at which to re-run that predicate. Either the room predicate is dropped — which leaks by construction — or the lease becomes `(principal × zone × moment)`, which is not the unification being claimed. Two further headline mechanisms fail on verification: *"expiry empties rather than marks"* is false at the site that matters (`identity.rs:88-95` clears the anonymous session only when `recency_ms < 5_000 && confidence ∈ [0.65,1.0] && !occupied` — an expired snapshot has `confidence = 0.0` and `recency_ms >= max_age_ms` per `publication.rs:96-104`, so it fails **both** guards); and *"write-time queue filling is strictly stronger than filtering at drain"* is orthogonal, not stronger — dropping the drain check is what creates the revocation window.

**What survives:** the storage-merge argument and the observation that the JARVIS coordinator is the better-engineered of the two writers. Both are kept below, without the premise.

### Proposal 2 is dead: a display is not a reader

The premise is *"under partition the two services degrade into stale readers, which is the same failure mode the privacy rule already demands."* A stale *reader* degrades to less context because it re-evaluates at the point of use. A rendered pixel is never re-read. Verified:

- `App.presence` is assigned at `crates/canvas/src/main.rs:2506` and there is no `self.presence = None` and no `.take()` anywhere in the file. Staleness is computed only inside the view (`main.rs:4838-4839`), and `presence::rail` passes `stale` to the three signal dots (`presence.rs:130-145`) while `occupant_view` draws `occupant.display_name` at full `palette.text` unconditionally (`presence.rs:109-111`).
- Stream content has no display expiry at all. `DeliveryPolicy::Ordered { max_age_ms }` filters at **ingestion** (`main.rs:1322`), and `DeliveryPolicy::Latest` has no age filter whatsoever (`main.rs:1311-1317`). Once applied to a widget spec, nothing removes it.

So partition makes Canvas hold a disclosure open with the evidence to revoke it on the far side of the break. Repairing that requires the renderer to own a fail-closed TTL per element — i.e. presentation owns a privacy decision — which contradicts the split the proposal is named after.

Two supporting claims also fail. *"Authorization becomes catalog narrowing… no new predicate, no new failure mode"*: `catalog` gates `desired.project_id` (`reconcile.rs:117-119`) and `ProjectResource`/`Terminal` resource ids (`lib.rs:945-969`), but `DataSource.owner` is a free string checked only for charset (`lib.rs:1013-1018`), `SurfaceContent::Stream { url }` is checked only for non-emptiness (`lib.rs:978-980`), and `SurfaceContent::Unsupported(Value)` is retained verbatim (`lib.rs:984-993`). And a narrowed catalog **is** a new failure mode: `WorkspaceStore::validate` validates every project document in the store against the effective catalog (`workspace_store.rs:130-139`) and `save` calls it (`workspace_store.rs:323`), so handing one actor's narrowed catalog to the writer fails the whole-store write.

**Correction to that attack:** it called `canvas_digest` "a live agency→presentation write." It is not live. `canvas_digest` appears in the tree only as `pub mod canvas_digest;` (`jarvis-subconscious/src/lib.rs:5`); nothing in `crates/` or `src/` calls `send`. The hazard is real in the contract — `envelope()` hardcodes `owner: "jarvis"` (`canvas_digest.rs:159`) and the payload carries `reference.title`, documented as the sender's own words — but it is latent, not executing. `inbox.rs` **is** wired (`intake_mcp.rs:23`, `src/bin/jarvis-intake-mcp.rs`).

**What survives:** `AspectKey<A>` with a crate-private constructor genuinely does make a cross-aspect write unrepresentable. That is a real property and a better answer to two-writer prevention than coordinating writers. It is kept as isolation rule 9.

### Proposal 3 is dead: the invariant is already false in code

The premise is *"only Credential evidence may authorize; Face and Voice may only route."* `DisclosureTact` is already the sole gate on private disclosure (`background.rs:872-885` above), and it is computed entirely from face/voice observations plus one static config boolean:

```
others_present = evidence.len() > 1 || any evidence with person_id == None   // fusion.rs:224-226
room_is_private = config.room_is_private && occupied && !others_present      // fusion.rs:239
tact = Direct  iff occupied && !others_present && config.room_is_private
                  && confidence >= config.direct_confidence                  // fusion.rs:240-243
```

A face match at 0.8 authorizes a private disclosure today. Second structural failure: `evidence` exists only for observations that arrived, so a silent unsensed person yields `others_present = false` and the room reads private. No lease can see someone the sensors never saw.

There is a third, sharper form. `resolve_voice_speaker` ignores `_presence` and `_zone` by construction (`identity.rs:13-29`) — correctly — but it *trusts its `speaker` argument*, and `ContextScope::new` uses the resulting `person_id` as the conversation-history map key (`identity.rs:37-48`). A voice score of 0.80 **is** a scope. Today the blast radius is bounded by the module doc — "Process-local session routing" (`identity.rs:50`) — and by `coordinator.rs:3` ("No sensor observations survive a process restart"). Any design that makes that key durable converts a transient mis-route into a permanent one.

**What survives, and it is the single strongest verified structural finding in the whole set:** after presence is factored out, Subconscious and Canvas share **no durable state**. `<state_dir>/coordinator.json` (`coordinator.rs:346-360`) and `workspaces.json` (`workspace_store.rs:272-289`) share no key space, no revision, no id. Nothing joins them. That is why the answer is a library, not a service.

### Proposal 4 survives — its premise, not its plumbing

The scope model, the splitting test, the refusal to reuse `usage_ledger::Scope` (verified: `contains` treats `None` as "any", `usage_ledger.rs:162-176` — the opposite default), and the presence-erasure prescription all hold. Three components do not:

- **§5 (the desktop-bridge broker) is dead.** `desktop_bridge_resolve` authenticates a bearer against a list of static credential-client secrets (`main.rs:4013-4026`, `credential_client_for_secret` at `9017-9026`) and then drives everything off `body.email`, a plain request field: `access_for_email(..., &email)` at `4035`, `can_use_scope(..., &email, ...)` at `4046-4053`. Nothing binds that email to the caller. A web tier holding the client secret can resolve anyone's bridge route and bearer.
- **§2.2's row key is dead.** `subject(config, email) = hex(sha256(public_url ‖ 0x00 ‖ email))` (`main.rs:11142-11148`). `Personal { subject }` is the email in a wig; a recycled address inherits the departed person's entire private store, and "no cross-subject API" cannot catch it because it is one key resolving to two humans.
- **§2.3 promotes `WorkspaceDocument` to a shared Project record without looking inside it.** `canvas_open_file` puts `path.file_name()` into `Surface.title` and `sha256` into `AssetRef` (`main.rs:6203-6217`, `1086-1106`), both persisted into `workspaces.projects[id]`. The splitting test was run against `WorkspaceStore`'s four top-level fields and never one level down.

### Corrections everyone got wrong

- **`/vouch` does not cost N round trips — it over-discloses on the first one.** `VouchResponse.scopes` is `vouch_scopes(...)` = `expand_grants(session.scopes)` plus the requested scope (`main.rs:4924-4941`, `11085-11100`). One call returns the subject's whole effective grant set. Read `ok`; discard the array. Proposal 3 stated the cost inverted.
- **`mcp:` is the *wrong* namespace and the right one is unprotected.** `requires_explicit_grant` covers the `mcp:` prefix, `.admin` suffix, `elmers.mcp`, `comfy.access`, `heimdall.node_operator` (`main.rs:11076-11083`), and is the only thing between a bare grant name and every allowed-domain address. But `fpl-auth/docs/cli-access-model.md:240-247` states plainly: *"The MCP model owns `mcp:*`. This model owns `api:*` and `infra:*`. An `mcp:` grant never authorizes an API call."* The documented namespace for a service grant is `api:<target>.<capability>` — and `api:` is not in `requires_explicit_grant`. Both proposals that leaned on `mcp:` were choosing a deliberate namespace violation without saying so. See §8.
- **Revocation for an internal address is a process restart.** `access_for_email` returns early for allowed-domain emails and reads grants from `config.member_grants` (`main.rs:10793-10800`), parsed once from a CLI argument at startup (`main.rs:1306`). There is no SIGHUP handler and no file watch — the only signal handling is terminate/ctrl-c (`main.rs:1379-1391`). `db.guest_grants_for_email` is consulted only for non-domain addresses.

---

## 3. The scope model

### Representation

```rust
/// One closed enum. No hierarchy, no wildcard, no Option that means "any".
pub enum ScopeRef {
    Personal { subject_id: SubjectId },                  // opaque, minted, never a hash of an email
    Team     { company_id: String, team_id: String },
    Project  { company_id: String, project_id: String },
}
```

Lives in one crate, `fpl-scope`, depended on by both repos. It appears in exactly four places: the enrollment/ACL tables, the envelope of every remote request, the row key of every durable record, and the constructor that gates `DesiredWorkspaceState`.

**Personal** = one enrolled human. `subject_id` is minted by the owning service and mapped from `(issuer, fpl-auth sub)` in a revocable enrollment row. It is **not** `sub` itself: `sub` is `sha256(public_url ‖ NUL ‖ email)`, so it is stable only while the issuer URL is, and it is recyclable with the address. Revoking enrollment deletes the mapping; a re-issued address then enrolls to a *new* `subject_id`. That is the only mechanism available — the hash is not revocable.

**Team** = the DB definition: `teams` + `team_memberships` carrying `company_id` and a per-member `role`. The config-only `FPL_AUTH_TEAMS` form (slug|display_name|members, no company, no role) is deployment bootstrap only. Both feed `session_principals`, and `principal_allowed_for_session` accepts either (`main.rs:10059-10072`). Pick the DB one before the first row is written.

**Project** = `(company_id, project_id)`, and **it is not derivable from fpl-auth today.** `projects_for_email` joins `company_memberships → companies → projects` and copies the *company* role down (`main.rs:6696-6719`); there is no `project_memberships` table; `project` is not a `Principal` kind (`principal_allowed_for_session` ends `_ => false`, `main.rs:10074`). Therefore:

> **Rule.** Project membership is an ACL table in the owning service, seeded by an explicit share, never derived from company membership. Until an explicit share row exists, a project scope resolves to **nothing** — not to the company's project list. Degrading "project" to "team" is not available either, because `projects` has no `team_id` and there is no join table.

### Who authorises what

Three answers, three owners, all three required, any one failing is a loud deny.

| Question | Answered by | Mechanism |
|---|---|---|
| Who is this bearer? | fpl-auth | `/vouch` → `{ok, subject, email}`. Read `ok` and `subject`. **Discard `scopes`.** |
| May this person do this verb? | fpl-auth | `/vouch?scope=<one grant>`; `ok` for that one scope only |
| May this principal touch this `ScopeRef`? | the service that owns the row | its own ACL table |
| Which service may write this row? | the row key | `AspectKey<A>`, constructor private to the owning crate |

fpl-auth structurally cannot answer the third: `can_use_scope(config, email, scopes, required_scope)` takes no object argument (`main.rs:11069`), matching is exact equality after trim+lowercase (`normalize_grant` `10488-10490`, `has_effective_scope` `11040-11045`), and `guest_grants`'s primary key is `email` alone. Do not encode an object into a grant string — `mcp:coordinator.read` does not imply `mcp:coordinator.read.jobs`, so every project would need a hand-maintained grant row.

### Sharing

1. A share writes **exactly one row**: `(ScopeRef, principal, capability)`.
2. No inheritance, no wildcard, no transitive expansion. Sharing a project grants that project and nothing else. Sharing an agent grants that agent and nothing else.
3. A share is revoked by deleting the row. Revocation must reach open subscriptions (rule 8), not only future ones.
4. `Principal` is `(kind, id)` matching fpl-auth's own pair — the precedent is `private_mcp_installations`, matched against `session_principals` by exact `(owner_kind, owner_id)` (`main.rs:5776-5791`, `2044`). Note that `principal_allowed_for_session` returns true for **every** principal when the caller holds `SSO_ADMIN_GRANT` (`main.rs:10055`, `10057`, `10072`); a domain SSO admin is therefore inside every scope. That is a property to state in the runbook, not one to discover.
5. Grant names: see §8. Whichever prefix is chosen, it must be in `requires_explicit_grant` **in the same commit that mints it**, or the domain wildcard hands it to every allowed-domain address and `vouch_scopes` reports it as held with no row recording it.

---

## 4. Where state lives

Three durable authorities, each already single-writer, each keeping what it has. No merge, no new service.

| Aspect | Owner | Store | Lock |
|---|---|---|---|
| `agency/*` — tasks, receipts, leases, attention, interruption ledger, briefings | JARVIS coordinator | `<state_dir>/coordinator.json` | `coordinator.lock`, "another coordinator owns this installation store" (`coordinator.rs:346-350`) |
| `presentation/*` — workspace documents, shared views, overlay policy | Canvas runtime | `workspaces.json`, validate/tmp-0600/fsync/rename (`workspace_store.rs:323-364`) | `flock` on `runtime.lock`, "Canvas runtime is already running" (`main.rs:6870-6881`) |
| `presence/*` — zone snapshots | jarvis-presence | publication file, one publisher per path | config refuses a second Presence publisher per zone |
| identity, verbs, enrollment | fpl-auth | its own DB | — |

**Two splits are required and are persistence-format changes:**

- `WorkspaceStore` fails the splitting test. `definitions` and `projects` are Project-scoped; `active_project_id` and `recent_project_ids` are Personal — and the comment at `workspace_store.rs:36-46` is explicit that the MRU trail exists to survive the human's own forgetting. A teammate may read a project's layout; nobody may read a browsing trail.
- `WorkspaceDocument` contents fail it too. `Surface.title` from `path.file_name()` and `AssetRef.sha256` are Personal facts inside a Project record. Use `SurfaceContent::ProjectResource { resource_id }` (`lib.rs:338-340`) for anything shared; a raw filename or digest never crosses a scope boundary.

**No replication, no consensus, no elected failover.** `distributed-topology.md` defers it, the code has only file locks, and `Enqueue`/`ClaimWork`/`FinishStep` have no worker client in-tree.

### Under partition

| Break | Behaviour |
|---|---|
| Canvas ↔ Presence | Every presence-derived element **expires to blank on Canvas's own clock**, per rule 5. Not greyed, not captioned — gone. |
| Canvas ↔ agency | Every rendered agency element expires at its `not_after` on Canvas's own clock. Canvas keeps presenting the document it owns and refuses document writes loudly. No local write-and-merge: `WorkspacePatch.base_revision` has no rebase operator and inventing one is a one-way door. |
| Subconscious ↔ Canvas | Deliveries needing a visual surface go `Held` — already the fail-closed default (`background.rs:873`). Voice continues under its own fenced claim. |
| Anything ↔ fpl-auth | No new scope may be opened. Existing leases run to expiry and are not renewed. Personal scope survives to the lease ceiling; **Team and Project scope do not survive a partition.** |

### Under revocation

Revocation must reach four places, and today it reaches none of them:

1. **Open subscriptions.** A subscription is bounded by a short lease that must be re-vouched; the object check is a per-frame lookup, not a table seeded at connect time.
2. **Resume cursors.** A `Last-Event-ID` cursor carries the ACL epoch it was issued under. A cursor older than the current epoch forces a snapshot restart, never a replay.
3. **The local catalog.** `effective_catalog` starts from `base.projects.clone()` and *pushes* any local definition not in the base (`workspace_store.rs:163-189`). It has no subtractive path, so a served catalog that has dropped a project is silently re-widened by the client's own stale `definitions` — and the store cannot simply drop them, because `validate` requires the effective catalog to cover every stored document. Revocation of a project currently never lands on a Canvas. This is the single worst verified revocation gap.
4. **Frozen authorization in the ledger.** `TaskAccess { thread_id, permitted_scopes, audience }` is written once at enqueue and never re-checked (`background.rs:28-33`). `task_access` gates on zone and `allowed_threads` and never consults `audience` (`coordinator.rs:1317-1325`).

---

## 5. Isolation rules

Numbered so they can be checked. Each is mechanical unless marked.

### 1. Assembled per scope, never produced-then-filtered

Every remote payload is constructed **from** a `ScopeRef` used as a query input. There is no code path that materializes an all-scopes value and narrows it afterwards.

*Check:* `Serialize` is implemented on the assembled projection type, never on the underlying row. The wide object is not serializable, so it cannot be accidentally emitted.

*Why this and not a filter:* every filter verified in this tree widens on its unknown branch. `overlay_on_viewport`'s `Leftmost` and `Rightmost` both end in `.is_none_or(|x| viewport.x == x)` and `Center` returns `true` outright when no display resolves (`main.rs:4970-5018`) — an overlay pinned to one display draws on **every** viewport when display identity is unknown. `PresenceWire::from_publication` maps every evidence row with no predicate and synthesizes `id: "unknown-{index}"`, `display_name: "Unknown person"` (`jarvis-presence/src/canvas.rs:196-226`). A filter's failure mode is a wrong predicate over a superset that still exists to be logged; an assembler's failure mode is a missing field, which fails loud.

> **Violations.** `RuntimeSnapshot` clones the full presence roster (`main.rs:3404`), every mounted shared view, and `workspace_summaries` over **every project in the store** (`control.rs:711-741`), and `mcp.rs:1012` serialises the entire `ControlReply` to text with no field allowlist. `ListBriefings` is gated on `Capability::Admin` alone and then returns `progress.reports()` — a bare `self.reports.values()` with no audience filter (`coordinator.rs:867-877`, `progress.rs:1018-1020`) — while the person-scoped door two hundred lines away refuses exactly that data (`progress.rs:847`, `886-900`).

### 2. Weaker evidence narrows scope; it never widens or creates one

Sensor evidence may select *which* endpoint an already-authorized scope materializes on. It may never produce a `ScopeRef`, add a capability, or open a disclosure.

*Check:* the function that turns evidence into a routing decision takes the evidence by `_`-prefixed parameter or does not take it at all, so no caller can widen it by supplying more. `resolve_voice_speaker(speaker, _presence, _zone)` (`identity.rs:13-29`) is the shape.

> **Violation.** `canvas-mac`'s `resolve_at` was cited by proposal 3 as the reference implementation for "every presence read." It is not: `identified` is `.any(|identity| identity.person_id == Some(config.person_id) && confidence >= min)` (`presence.rs:212-219`) — **any**, not **only** — its `Snapshot` does not deserialize the `privacy` block at all (`presence.rs:56-62`), and "ambiguous" means `displays.len() != 1` (`presence.rs:236-240`), which is ambiguity across monitors, not about who is in the room. It answers "which monitor is this person at." Keep it as a router. `background.rs:875-885`'s `.all(...)` is the audience gate and must not become `.any(...)`.

### 3. Unresolved identity yields a different, smaller object — not the same object blanked

An unresolved viewer gets a distinct empty session, not a redacted one. The precedent is `ContextScope { zone, person: Option<String> }` (`identity.rs:32-48`) — a separate map key, with a 15-minute anonymous idle fuse (`ANONYMOUS_IDLE_TIMEOUT_MS`, `identity.rs:10`, `prepare_history_scope:103-136`).

*Check:* the assembler for an unresolved principal and the assembler for a resolved one return different types.

> **Violations.** `session_context_message` pushes `config.session_context` as "Ambient context" with no speaker or scope test at all, and falls back to `config.project_context` when the request carries none (`lib.rs:321-338`) — while `session()` clones `self.config` into every per-scope provider (`identity.rs:368-372`). Ambient notes and project context therefore reach an unrecognised speaker in the same turn where conversation history correctly does not. Both halves have the same defect; deleting only the `project_context` fallback fixes half of it.
>
> The audience-change reset is worse. `clear_anonymous` fires only when `!audience.is_empty() && audience.len() == snapshot.observed_identities.len()` (`identity.rs:264`) — every observed identity must have resolved confidently. One unenrolled face makes the lengths differ, the comparison is skipped **entirely**, and the anonymous session continues in front of the new arrival. The whole block is additionally nested inside `presence.filter(|s| ... s.occupied ...)` (`identity.rs:213-219`), so an expired snapshot (`occupied = false`) cannot reach it either. Invert it: clear **unless** the roster is fully resolved and unchanged.

### 4. Expiry erases identity at the store, not at the renderer

Past `max_age_ms`: occupants dropped, `occupied` false, identities cleared, disclosure reserved — before the value is read by anything.

The correct implementation already exists: `PublishedPresence::snapshot_at` sets `occupied = false`, `confidence = 0.0`, `observed_identities.clear()`, `others_present = true`, `room_is_private = false`, `tact = Reserved` (`publication.rs:96-103`).

> **Violation, and it is on the path that matters.** The Canvas socket path never calls `snapshot_at`. `jarvis-presence/src/main.rs:239` hands the raw publication to `canvas.publish_presence`, and `PresenceWire::from_publication` reads `result.snapshot.occupied` and `result.evidence` directly (`canvas.rs:177-194`), shipping `expires_after_ms` for the consumer to honour. Canvas never honours it: `self.presence` is set at `main.rs:2506` and never cleared; staleness is a render-time comparison (`main.rs:4838-4839`) that recolours a caption and three dots. The erasure that exists is bypassed by the one consumer that most needs it.

### 5. Every persisted emission carries a `not_after`, enforced by the renderer's own clock

Speech ends by itself. A screen does not. Any element rendered from another aspect's data — presence, briefing, digest, camera still — carries an absolute `not_after` and the renderer blanks it on its own monotonic clock, with no input from the producer.

*Check:* a rendered element type without a `not_after` field does not compile.

*Why:* this is the only rule that survives a partition, because it is the only one whose evidence is local. It is also the fix for the two-audience-algebra problem: it is the room predicate re-evaluated continuously, which is the only form a screen can support.

> **Violations.** `DeliveryPolicy::Latest` has no age filter (`main.rs:1311-1317`) and `Ordered { max_age_ms }` filters at ingestion only (`main.rs:1322`); once applied, nothing expires it. `PresencePreview::validate` requires only `!self.faces.is_empty()` (`control.rs:497-506`) — a *detected* face suffices, identification is not required — and the only thing that removes a delivered frame is the sender's own `linger_ms`, bounded 250–30 000 ms. `Command::DismissPresencePreview` is handled at `main.rs:2566` and **nothing in the entire jarvis tree emits it** (grep: zero hits). Note the asymmetry: `presence_preview` does auto-evict on `linger_ms` (`main.rs:818-822`). The camera still expires; the names do not.

### 6. No content-addressed lookup authorizes by possession

Any cache, dedup, or asset key that crosses a scope boundary is keyed by `(ScopeRef, content)`, not content alone. Cache residency is never an authorization check.

> **Violation.** `background_assets: BTreeMap<String, Handle>` is keyed by digest alone (`main.rs:370`), inserted at `1163`, and never pruned — `prune_decoded` does not touch it. `ensure_background_available` returns `Ok` iff `self.background_assets.contains_key(&asset.sha256)` (`main.rs:3824-3833`), so residency **is** the check, and the error quotes the missing digest verbatim — an oracle over which files have been imported. The digests are handed out: `SurfaceSnapshot.asset_sha256` and every `workspace_summaries[].background` ride in `RuntimeSnapshot`. A principal holding a shared project can read a digest from one project's snapshot and set it as another project's background.

*Related, and stated as a hazard rather than a verified store:* `prechange.rs:231-233` derives `jarvis:prechange:{sha256(text)[..20]}`, `presentation.rs` derives a TTS key from text alone, and `intake_mcp.rs:502` **does** scope its key as `short_hash("{sender}\0{key}")` — the discipline exists in the same file and is not applied to the TTS keys. I could find no cache implementation for those keys in either tree; they are contract fields. Prefix them before anything implements one.

### 7. A scoped document contains no free-form pointer

Anything in a document that names a data source, endpoint, or address is validated against the scope-resolved catalog, or it is not allowed in the document.

> **Violations.** `DataSource { owner, stream, contract }` is checked for identifier charset only (`lib.rs:1013-1018`); `SurfaceContent::Stream { url }` for non-emptiness (`lib.rs:978-980`); `SurfaceContent::Unsupported(Value)` for being a JSON object (`lib.rs:984-993`). `ProjectResource` and `Terminal` **are** checked against `project.resources` (`lib.rs:945-969`) — that is the right shape and the other three should match it.

### 8. Authorization is checked at drain and at resume, not only at admission

Write-time scoping and drain-time re-authorization are both required; they are orthogonal, not redundant. A resume cursor carries an ACL epoch; a cursor from before the current epoch forces a snapshot restart, never a replay.

> **Violation-in-waiting.** `StreamInbox` is one global `VecDeque` capped at 512 with drop-oldest and a counter (`stream_binding.rs:71-89`), unpartitioned by owner or stream — one scope's traffic evicts another's, and `drain(limit)` hands whatever is at the head to whoever calls.

### 9. A cross-aspect write is unrepresentable, not merely forbidden

Every durable row is keyed `AspectKey<A>` whose constructor is private to the owning crate. Canvas cannot name an `agency/` row because there is no constructor.

*Check:* `grep` for the constructor outside its crate returns nothing, enforced by visibility rather than review.

### 10. Producer identity is verified, never asserted

An envelope's `owner` field is a claim. It is checked against the authenticated producer or the frame is dropped.

> **Violation.** `StreamEnvelope.owner` is validated for charset only (`stream_binding.rs:41-49`); the socket is a `UnixDatagram` at mode 0600 with a bare `recv` and no per-message peer credential (`stream_binding.rs:136-152`). Any same-uid process writes any owner and drives a widget on any project's surface. This is an integrity leak today and a confidentiality one the moment the transport leaves the machine.

### 11. Transport is never the authorization model

`verify_peer` is a uid comparison (`canvas-control/src/lib.rs:150-180`); `loopback.rs:12-14` says it outright — *"A bearer on loopback is not authentication of a person."* Same-uid is the account, and the account contains agents holding narrower scopes than the human.

> **Violation.** `canvas_screenshot` with no `surface_id` captures `x = params.x.unwrap_or(snapshot.frame.x)`, `width = params.width.unwrap_or(snapshot.frame.width)` — the entire frame — bounded only by 1..=16384 px (`mcp.rs:826-842`), with `verify_surface_chrome` skipped and `if !snapshot.visible` as the only gate (`mcp.rs:769-771`). It reads pixels, so there is no payload to assemble from a scope key: rule 1 cannot reach it. Separately, `canvas-mac/src/remote.rs` reaches a project desktop over loopback VNC at `127.0.0.1::{15900 + output_index}` (`remote.rs:168`, `349`) keyed by output index, with one machine-wide password read from `~/.config/dock/wayvnc/password` (`remote.rs:276-292`). A closed enumeration of project ids cannot narrow a port.

### 12. `reconcile.rs` stays pure and stays co-located with the applier

No scope argument, no actor field, no I/O. Its doc comment (`reconcile.rs:1-5`) and its signature (`reconcile(&WorkspaceDocument, &DesiredWorkspaceState, &Catalog)`) are the contract. Every revision is `current.revision + 1` (`:156`, `:179`) and the self-check `require(applied == target, ...)` (`:158-163`) is meaningful only against a byte-identical local `current`.

Three consequences, all accepted: writes serialize per document and a losing client **re-plans** rather than retrying a patch; the catalog becomes a scope-versioned served artifact (see the revocation gap in §4); and authorization happens *before* reconcile, in a `ScopedIntent` that is the only constructor of `DesiredWorkspaceState`. Do not hand `reconcile` a narrowed catalog as the authorization mechanism — the narrowed catalog cannot be persisted (`workspace_store.rs:130-139`, `323`) and it does not reach `source.owner` anyway (rule 7).

> **Violation, unrelated but on the same surface.** `ControlRequest::validate` loads an operational view from the **caller's** config before the message is sent (`control.rs:1013-1015`). That validates the wrong file the moment caller and runtime are different machines.

---

## 6. One-way doors

1. **`ScopeRef`'s shape** — no hierarchy, no wildcard, no `None`-means-any. It lands in every envelope, every row key, every ACL entry. The failure mode of getting it wrong is silent widening years later. Deciding which `Team` means (DB teams with `company_id` and role, versus config-only `FPL_AUTH_TEAMS` with neither, both of which feed `session_principals`) must happen before the first row is written.
2. **`Personal`'s key.** Minting an opaque `subject_id` with a revocable enrollment mapping is correct and re-keying a populated store afterwards is a migration under a live lease. Keying on fpl-auth `sub` is the cheap option and is unrecoverable, because `sub` is a hash of a recyclable email under a rotatable `public_url`.
3. **The grant namespace.** `api:` is documented as correct (`cli-access-model.md:240-247`) and is *not* in `requires_explicit_grant`; `mcp:` is protected in two independent places but is documented as belonging to a different model. Whichever is chosen, the `requires_explicit_grant` entry must land in the same commit, and renaming later is a revocation event for every subject at once — grants are flat strings keyed on email with no alias mechanism.
4. **Splitting `WorkspaceStore`** into Personal (`active_project_id`, `recent_project_ids`) and Project (`definitions`, `projects`) halves, and moving raw filenames and digests out of `WorkspaceDocument`. Both are persistence-format changes to a file installed on machines, guarded by owner-only checks and a whole-file atomic rewrite.
5. **Making the catalog a served, scope-versioned artifact** and giving `effective_catalog` a subtractive path. Until it has one, revocation does not land; after it has one, every existing store must be migrated, because `validate` currently requires the effective catalog to cover every stored document.
6. **Presence control protocol v2.** `ControlRequest`, `Command` and `PresenceSnapshot` are all `deny_unknown_fields` (`control.rs:48`, `58`, `418`) with a hard `schema_version` reject. There is no additive escape and no staged rollout: an un-upgraded Canvas rejects a new command outright rather than ignoring it. It fails closed, so it is not a leak — but the ordering (upgrade every Canvas before any producer emits) belongs in the runbook.
7. **Adding a scope/project dimension to `Audience`.** Today it is `Public | Private(String) | Held` (`background.rs:35-40`), so a briefing shared with a group must be `Public`, and `compile_pending_matching` merges every pending update in a zone with that audience into one report regardless of producer or project (`progress.rs:504-546`). Fixing that is a wire change to the ledger.
8. **Never rebasing `WorkspacePatch`.** Serializing writes per document is easy to reverse. Publishing merge semantics for `base_revision` and then withdrawing them is not.
9. **`AspectKey<A>`'s partition** — which service owns which prefix. Moving an aspect later means migrating durable state out from under a live lease on two machines at once.

---

## 7. First move

**Make Canvas's presence store erase identity on a tick rather than recolour it at render, and write the two tests that do not exist.**

All four proposals independently arrived here. That convergence is the argument: it is the only change that is required under every one of the four topologies, closes a leak verified by reading, needs no network, no new crate, no wire change, and no persistence-format decision.

1. **Write the failing tests first.** No test anywhere asserts that stale or unresolved presence stops a name being drawn. `crates/canvas/src/presence.rs` has exactly one test and it is about crop geometry (`presence.rs:348-361`). `main.rs:7816` asserts the *opposite* property — `assert_eq!(app.snapshot().presence, Some(snapshot))` pins that a snapshot round-trips through Canvas unchanged. On the JARVIS side the equivalent narrowing is tested three times over. The asymmetry is Canvas-only. Add `stale_presence_draws_no_name` and `unidentified_occupant_draws_no_name`, watch them fail, then invert the round-trip assertion.
2. **Evict on the tick.** Move the `received.elapsed() >= expires_after_ms` comparison out of the view (`main.rs:4838-4839`) into the existing tick, and *drop* the snapshot from the struct. `RuntimeSnapshot.presence` is already `Option<PresenceSnapshot>` (`control.rs:728`), so this removes the roster from `Inspect` and from the MCP echo **with no schema change** — `deny_unknown_fields` and the hard `schema_version` check are not disturbed.
3. **Drop the name when the signal is not `Verified`.** `occupant_view` draws `display_name` at full `palette.text` regardless of `stale` (`presence.rs:109-111`). Render the body without the name, which also finally gives `PresenceSignalState::Unknown` (`control.rs:389-396`) a meaning — it exists in the protocol, no producer emits it, and the renderer collapses it into the inactive branch.
4. **Gate the preview on identification, not detection.** Require at least one face carrying an `occupant_id` in `PresencePreview::validate` (`control.rs:497-506`).

That is one PR. It closes the widest currently-verified leak, and it forces the first honest encounter with rule 5 — an emission that expires on the renderer's own clock — at a scale where getting it wrong costs an afternoon.

**Second move, only after that lands:** extract `fpl-scope` and one envelope crate. `jarvis-subconscious/src/canvas_digest.rs:32-41` already hand-copies `StreamEnvelope` from `canvas/src/stream_binding.rs:25-35`, and `jarvis-presence/src/canvas.rs:22-24` re-declares `CONTROL_VERSION`, `MAX_CONTROL_MESSAGE` and `MAX_PREVIEW_BASE64_BYTES` alongside a hand-copied `PresenceWire`. That duplication has to die before either side grows a scope field, because it is a security-relevant schema maintained in two places.

**Deliberately held back:** tightening `overlay_on_viewport`'s three `is_none_or` branches (`main.rs:4977-5017`). The fix (`is_none_or` → `is_some_and`, `frames.is_empty() => false`) is correct fail-closed behaviour, but I did not verify whether `selected_display_ids` is routinely populated in normal single-display operation. If empty is the normal case, that change silently disables all overlays rather than narrowing them, and the right fix is to resolve to the actual single viewport. Verify first.

---

## 8. What remains unresolved

**The grant namespace is a genuine conflict between two verified sources.** `fpl-auth/docs/cli-access-model.md:240-247` says the MCP model owns `mcp:*`, this model owns `api:*` and `infra:*`, and *"an `mcp:` grant never authorizes an API call."* The documented shape is `api:<target>.<capability>`. But `requires_explicit_grant` (`main.rs:11076-11083`) protects `mcp:` and does not protect `api:`, so the documented-correct namespace is exactly the one `can_use_scope`'s domain wildcard hands to every allowed-domain address implicitly. Neither option is right as things stand. The fpl-auth change (adding `api:` to `requires_explicit_grant`) is small, but it must land **before** the first grant is minted, and I did not check whether existing `api:` grants are relied on by callers who would break when the wildcard stops covering them. That check is a prerequisite and I have not done it.

**Personal-key recycling has no clean fix.** Minting an opaque `subject_id` behind a revocable enrollment row is the best available answer, but it only helps if enrollment revocation is actually performed when someone leaves. If it is not, the mapping resolves the recycled address to the departed person's `subject_id` and the outcome is identical. This is a process dependency dressed as a technical control, and I would rather name it than pretend otherwise.

**I cannot verify how `fpl-desktop-bridge` scopes its route token.** That binary is not in `dock`, `jarvis`, or `fpl-auth`. `desktop_bridges` has exactly one credential column, `route_token_ciphertext` (`main.rs:5920-5933`), and `DesktopBridgeCapability` carries no credential of its own — `capability_id` only selects a URL path. So a resolved token appears to be device-wide rather than capability-wide, and `revoked_at` filtering (`main.rs:6947-6957`) stops future resolves without invalidating an issued one. Whether the bridge itself narrows per connector is unknown from here. Do not build on that endpoint until someone reads that repo.

**`context_snapshot` has a reader and no writer.** `intake_mcp.rs:465` reads a JSON file carrying per-recipient `available`/`attentive`/`dnd`/`visual_surface`/`active_project_id` — presentation and presence facts crossing into agency through a file. I searched both trees and found no writer. Either it is written by something outside these repos or the path is dead. Until that is answered, the aspect ownership of those fields is undecided.

**Briefings have no project dimension, so a project-scoped lease cannot narrow one.** `ProgressLedger` keys everything by `(zone, Audience)` — `recipient_view` and `recipient_detail` filter on `Audience::Private(person)` (`progress.rs:847`, `886`) and refuse cross-recipient resolution, which is genuinely good. But there is no project axis anywhere in the ledger, and `Audience` has no group variant, so a briefing about a shared project must be `Public`, and `compile_pending_matching` merges by `(zone, audience)` alone (`progress.rs:504-546`). Assemble-per-scope has nothing to assemble from here. This needs an `Audience` variant and a compaction key change; I have not designed either.

**The right `not_after` for a rendered element is not derivable from anything in these repos.** Rule 5 says every persisted emission carries one. What that value should be for a wall display versus a laptop versus a phone is a judgement about rooms, not a fact in code, and it is the one rule in §5 that a person has to set rather than a test.

**The blast radius of `SSO_ADMIN_GRANT` is stated but not designed around.** `principal_allowed_for_session` returns true for every principal when the session holds it (`main.rs:10055`, `10057`, `10072`). Every ACL row keyed on `(owner_kind, owner_id)` — including a person's `Personal` scope holding voice transcripts — is therefore reachable by a domain SSO admin. Whether that is acceptable is a policy question I am not in a position to answer, but it must be answered before the Personal store holds conversation turns.

**One rule in §5 is not mechanical.** Rule 5's *correctness* — that a renderer blanks on its own clock — is checkable. Whether a given element is "an emission from another aspect" and therefore needs a `not_after` at all is a judgement at the type boundary. That is the one worth spending review time on.

---

| Rule | Enforcement |
|---|---|
| 1 Assembled per scope | type system: `Serialize` only on the projection, never on the row |
| 2 Evidence narrows | signature: routing functions take evidence by `_` or not at all |
| 3 Unresolved ⇒ different object | type system: distinct return type for the unresolved principal |
| 4 Expiry erases at the store | test: an expired publication yields no identities from the store, not the renderer |
| 5 Emission `not_after` | type system: rendered element without `not_after` does not compile |
| 6 Scope-keyed content addressing | test: identical content in two scopes produces two keys; residency is never a check |
| 7 No free-form pointer | test: every source-naming field is validated against the catalog |
| 8 Drain + resume re-auth | test: a cursor older than the ACL epoch returns a snapshot restart |
| 9 Cross-aspect write unrepresentable | visibility: constructor private to the owning crate |
| 10 Producer verified | test: an envelope whose `owner` differs from the authenticated producer is dropped |
| 11 Transport is not authorization | review, at each new surface |
| 12 `reconcile` pure and co-located | `canvas`'s reconcile module depends on no I/O crate; `ScopedIntent` is the only constructor |