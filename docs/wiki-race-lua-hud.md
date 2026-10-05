# Lua-native wikiracing HUD

The TV workspace uses `config/modules/wiki-race-v1.{json,lua}`. Its layout,
clock formatting, status, click count, trace, and START/STOP button policy are
Lua. Rust owns scene validation, themed rendering, credentials and transport.
The race controller owns navigation, holds, click counts and both elapsed times.
Surf remains a generic browser; no wikiracing behavior was added to Surf.

## Runbook

1. Build Canvas with `lua-scenes` enabled. For the Pi, cross-build on Agora One.
2. Install the module pair and `config/binders/wiki-race-state-v1.lua` under the
   Canvas runtime config directory. They hot-reload without another build.
3. Use `config/workspaces/tv/wikiracing.json` for the HUD plus `surf-tv` stream.
4. Give the Canvas process `CANVAS_MODULE_BUS_CONFIG=/path/to/client.json`, an
   existing Unibus ClientConfig with a **distinct** producer ID, e.g.
   `canvas-wikiracing-tv`. Store credentials in its token file or token env,
   not in the workspace or Lua. Connection happens on a background thread.
5. Grant that principal permission to send `wiki.race.control.v1` to the
   controller and receive its snapshots and receipts. The default controller
   destination is `wiki-race-controller`, configurable in the module manifest.
6. The controller publishes the state below and consumes the control contract.
   The HUD is not live until that adapter is connected. Without it the widget
   says disconnected/stale and its buttons are disabled; no demo state is made up.

## State ingress

Publish `canvas.widget.snapshot.v1` with the following **message body** (no
transport token). Alternatively send the same StreamEnvelope to Canvas's
existing local stream socket. Both paths use the same binder and validation.

```json
{
  "schema_version": 1,
  "owner": "surf",
  "stream": "wiki-race",
  "contract": "canvas.widget.snapshot.v1",
  "sequence": 1,
  "emitted_at_ms": 1780000000000,
  "project_id": "wikiracing",
  "payload": {
    "race": {
      "schema_version": 1,
      "race_id": "race-1",
      "revision": 7,
      "phase": "racing",
      "start_page": "Kevin Costner",
      "target_page": "Nebuchadnezzar",
      "current_page": "The Matrix",
      "clicks": 4,
      "race_elapsed_ms": 12340,
      "wall_elapsed_ms": 15340,
      "observed_at_ms": 1780000000000,
      "trace": "Selected an article link"
    }
  }
}
```

Use current timestamps and monotonic sequence/revision numbers. Phases are
`ready`, `racing`, `holding`, `finished`, `stopped`. Race elapsed must not exceed
wall elapsed; the controller excludes presentation holds. The HUD does not
advance/fabricate elapsed time between snapshots. Publish at a modest cadence,
e.g. 4 Hz while racing and 1 Hz while idle. After five seconds without a fresh
controller observation, controls disable. Finished states display FINAL TIME.
An invalid snapshot retains the last validated projection.

## Controls and receipts

Only an enabled user activation emits `wiki.race.control.v1`:

```json
{
  "schema_version": 1,
  "action": "start",
  "race_id": "race-1",
  "expected_revision": 7,
  "start_page": "Kevin Costner",
  "target_page": "Nebuchadnezzar"
}
```

`stop` uses the same shape. The owner must reject stale revisions, enforce its
own start/stop state machine, and deduplicate by the transport request ID. It
replies to the command producer with `canvas.module.action.result.v1` and body
`{"request_id":"<command envelope ID>","ok":true}` (or `ok:false`). Canvas
accepts only a correlated receipt from the addressed controller. A receipt
timeout means outcome unknown, not failure or permission to replay. The
controller's subsequent state snapshot determines the displayed race state.

Reload and initial load never emit commands. Activations re-evaluate the latest
Lua artifact against the latest inputs. Scripts cannot select an arbitrary
destination, kind, endpoint or credential: the artifact manifest declares the
routes, bodies are limited to 4 KiB, and router grants still enforce authority.
The host bounds pending receipts and disables transport controls on disconnect
or inbound queue drops; reconnect/resynchronize explicitly.

## Verify

```sh
cargo check -p canvas --features lua-scenes --lib --bin canvas
cargo test -p canvas --features lua-scenes --test wiki_race_hud
cargo test -p canvas --features lua-scenes --lib
```

The migration leaves the legacy Rust wiki widget kinds available for older
documents, but the wikiracing workspace does not use them. Rendering, theme
primitives and the action boundary are reusable by other Lua-native widgets.
