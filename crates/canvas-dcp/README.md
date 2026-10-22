# Canvas DCP provider

The `canvas-tv` provider runs on MacBook Neo and controls the physical Fire TV through
`canvas-tvctl`. Its workspace catalog comes from the TV's `inspect` response. The
provider checks catalog/state revisions before dispatch and verifies the selected
document with another inspection before acknowledging the DCP action. Workspace
state revisions cover the active project, installed projects, visibility and
owner. Renderer ticks and widget focus remain observable but do not invalidate a
workspace switch. Native Mac dispatch needs no renderer revision precondition.
The TV owner still requires an endpoint revision at dispatch. The adapter calls
its existing `workspace switch` CLI, which obtains that CAS on the same owner
connection; a concurrent TV change rejects the action rather than overwriting it.

## Install and check

1. Install `adb` and `canvas-tvctl` on Neo. Check Neo's authorized ADB connection reports
   `192.168.2.97:5555 device`.
2. Install `canvas-dcp` and `config/deploy/macbook-neo/dev.fpl.canvas-dcp-tv.plist`.
   The LaunchAgent sets `CANVAS_DCP_ANDROID_MODE=workspace`,
   `CANVAS_DCP_ANDROID_SERIAL=192.168.2.97:5555`, and the `canvas-tv` identity.
3. Query `/dcp/v0.1/catalog` on `127.0.0.1:7895` with the local Canvas bearer
   token and `DCP-Version: 0.1`. Confirm `security` appears in the
   `project_id` enum and `provider.instance` is `urn:fpl:canvas:firetv-omni`.
4. Execute `canvas-tv.workspace.activate` with the returned catalog and state
   revisions. Query the catalog again and confirm `active_project_id` is the
   requested project. The TV itself remains the final source of truth.
5. Stop `unibus-dcp-canvas.service` and `canvas-dcp.service` on home-pi before
   enabling Neo's UniBus publisher. Publish from Neo through its existing
   router tunnel so only one live publisher owns `canvas-tv`.

The Android audio provider remains available with
`CANVAS_DCP_ANDROID_MODE=audio` and its own provider ID and listener port.

The same binary controls native Mac Canvas through its existing control socket
when `CANVAS_DCP_ANDROID_SERIAL` is absent. Use distinct provider IDs, instances
and listen ports for native and TV surfaces; the Neo native installation uses
`canvas-neo`, `urn:fpl:canvas:macbook-neo`, and `127.0.0.1:7896`. No Accessibility
helper or permission-bearing application is replaced.
