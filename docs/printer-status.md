# Lab printer status

The read-only Bambu slice lives across a narrow ownership boundary:

`Bambu MQTT/TLS → unibus-bambu → printer.status.v1 → canvas-printer-observer → Lua primitives`

Unibus owns the shared model (`unibus-device::printer`), protocol reduction and
private printer credentials. Canvas only checks enrolled source/device identity,
schema bounds and freshness, then renders typed inputs. Mycelium discovery is
optional evidence, not an authorization grant. Physical controls are not exposed.

Assets:

- `config/modules/printer-status-v1.{json,lua}`: themed read-only module.
- `config/binders/printer-status-v1.lua`: typed snapshot projection.
- `config/workspaces/tv/printers.json`: three cards, initially awaiting enrollment.
- `config/deploy/printer-observer.example.json`: explicit device/producer mappings.

Install the Lua manifest/source under the target Canvas runtime `modules/`
directory. Register project `printers` before importing the workspace. Replace
placeholder binding streams with `printer-` plus the first 32 hex characters of
SHA-256 of the exact device ID (`bambu:<serial>`). Install the observer beside
Canvas and configure restricted Unibus grants/routes and an authenticated
loopback tunnel when needed. Do not assume an addressed status stream broadcasts
to all displays.

Build/verify on Agora:

```sh
cargo check -p canvas --features lua-scenes --bin canvas-printer-observer
cargo test -p canvas --features lua-scenes --test printer_status_hud --bin canvas-printer-observer
cargo test -p canvas --features lua-scenes --lib
```

The UI hides stale numeric fields individually, and expires a snapshot after six
seconds even if its source stops without disconnecting. No measurements or jobs
are invented. Each adapter instance subscribes only to its enrolled printer's
report topic; printing/motion/temperature commands are intentionally absent.

Enrollment and private credential/TLS-pin setup:
[Unibus runbook](https://github.com/FuturePresentLabs/unibus/blob/main/docs/bambu-status-runbook.md).
All three lab printers still need enrollment; tracked by `dock-0z2`. Code/widget
work `dock-b0w` is complete, verified on Agora—not a claim of live deployment.
