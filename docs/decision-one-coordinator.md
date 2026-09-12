# Decision: Canvas Backend and JARVIS Subconscious Are One Coordinator

> **Status:** Resolved
> **Bead:** dock-ybc.10
> **Date:** 2026-09-12
> **Constraint:** This is the genuine one-way door. The transport envelope and
> presence routing state must be unified before either side builds transport.

## Context

Two documents — `jarvis/distributed-topology.md` (V1 contract for jarvis-8uz.10)
and `dock-qwv` (Canvas backend design) — independently describe the same system:
an authoritative per-installation coordinator that owns cache and state, with
sensors/daemons registering and publishing snapshots, and screens attaching as
subscribers.

## Decision

**One coordinator.** Canvas's backend and JARVIS Subconscious fuse into a single
per-installation service. Neither maintains its own presence-derived routing
state; both consume presence from a single upstream source.

## Rationale

1. **Presence has one owner.** The topology doc states "Presence owns sensing
   and fusion." dock-cdz already renders zone presence with independent
   Presence/Face/Voice evidence. Presence is upstream of both Canvas and
   JARVIS; neither should derive it independently.

2. **Split-brain risk.** The topology doc warns: "multiple coordinators must
   not independently write the same installation's state." If Canvas's
   backend and the Subconscious coordinator are separate services both
   holding presence-derived routing state, this is exactly the failure mode
   to avoid. It is cheap to avoid now and expensive after both have built
   their halves.

3. **Shared transport envelope.** The envelope carries: installation ID, schema
   version, message ID, producer ID, observation timestamp. Canvas's stream
   contract and JARVIS's fact/command payloads should not invent two
   incompatible envelopes.

4. **Mobile/browser targets (dock-8nz) make consolidation attractive.** A phone
   should attach to one service, not federating between two.

## Constraints Satisfied

- Presence has ONE owner — Canvas's backend consumes presence rather than
  deriving it.
- Single transport envelope — `StreamEnvelope { schema_version, owner, stream,
  contract, sequence, emitted_at_ms, payload }` is used by both.
- Mobile/browser targets attach to one coordinator.

## What This Means for Canvas

- The Canvas control socket (`stream.sock`) IS the coordinator's event channel.
- JARVIS Subconscious federates through the same envelope: presence facts,
  conversation state, and delivery receipts are emitted as stream events
  with `owner: "jarvis"` and appropriate contracts.
- Canvas surfaces render presence-routed data by subscribing to streams,
  not by querying JARVIS directly.
- The `Command::SetPresence` control command is the single write path for
  presence — both Canvas and JARVIS go through it.

## What This Does Not Mean

- Canvas's rendering role is unchanged — it remains the workbench, JARVIS
  remains the brain. The division of labor (Subconscious routes conversation
  and delivery; Canvas routes surfaces) is preserved.
- This is an architectural decision, not an implementation. The unification
  happens in the shared Alfalfa process that owns presence, not in either
  Canvas or JARVIS alone.
