# Owner-managed desktop enrollment

Canvas invokes FPL Desktop Bridge rather than implementing an identity issuer or
copying another machine's credentials. The owner CLI now provides:

```sh
fpl-desktop-bridge status
fpl-desktop-bridge pair --open false
```

Both accept `--config PATH` after the subcommand. Status is read-only JSON with
`schema_version: 1`. `session: authenticated` means the owner verified its OS
keyring application session against `/desktop-bridges/connections` now. Missing
credentials, an inaccessible credential store, issuer failure, and expired or
revoked sessions are distinct. Stored email and bridge-token presence do not
prove validity. `bridge_registration` and `browser_identity` remain
`not_verified`; this endpoint makes no assertion about either.

Pairing emits flushed newline JSON. `approval_required` contains
`verification_uri_complete` and `expires_in`; the private polling proof never
leaves the owner. After actual browser consent, the owner writes its registration
and application session to its OS keyring and emits `paired` with device ID and
email. A polling failure emits `failed`; the caller must also handle nonzero
exit, EOF, startup failure, and timeout. Killing and waiting for the child stops
polling; simply dropping a Rust Child does not. A new attempt creates a fresh
pairing request.

For Agora, install the existing owner source and build its non-GUI binary with
`cargo build --locked --bin fpl-desktop-bridge`. Ubuntu requires
`libdbus-1-dev` and `pkg-config`. Run `scripts/setup-agora-bridge.sh` on Agora;
it installs the binary and an SSH-safe launcher using the original user's
Secret Service bus. Initial configuration derives its device ID from the host
and route from existing Tailscale, disables example connectors, and preserves
any existing owner configuration. It does not start a daemon or enroll a device.

Open the public approval URL in Agora's persistent Firefox profile and let the
user sign in and consent there. That both authenticates that browser and approves
the bridge application. Opening it on the Mac approves the application but does
not give Agora's Firefox an SSO cookie. No supported cross-machine cookie handoff
was found in the owner contract. Browser SSO and bridge application identity must
be verified separately after approval. An enrolled bridge also does not establish
that a selected Edgerunner session has working Bifrost or MCP authorization.

The Canvas client is `crates/canvas-mac/src/bridge.rs`. Mac invocation honors
`DOCK_DESKTOP_BRIDGE_BIN`; Agora uses the existing SSH host and
`.local/bin/fpl-desktop-bridge`. No credential or SSH-key transfer is involved.

Canvas exposes the owner interface directly:

```sh
canvas-mac bridge status agora
canvas-mac bridge pair agora studio
```

The pair command opens the owner's public approval URL in the named project's
persistent Agora web profile, then waits up to the enrollment deadline. Complete
SSO and device approval in that streamed browser. The existing demo Firefox
profile is preserved; it does not acquire cookies from the new web profile.
Different project web profiles also do not automatically share SSO cookies.
The initial live handoff displayed the real FPL sign-in page; enrollment is not
complete until the owner reports `paired` and a fresh status verifies the session.

The installed owner CLI has been built and tested on both Mac and Agora. On
Agora, the authenticated GUI loads the desktop catalog, owns its child bridge
daemon, and configures the detected OpenCode client automatically. Edgerunner
session binding remains distinct from device enrollment and MCP readiness.

## Project desktop MCP

`canvas-linux mcp-serve PROJECT PORT` uses the Rust MCP SDK and exposes only that
project's status, PNG capture, recall, focus, text, key and click operations.
Recall takes no project argument and requires agent ownership. It invokes
Canvas's existing Sway APIs and human takeover gate. There is no grant-control,
arbitrary shell, or agent-session provisioning tool. Status filters other
projects out; capture requires the requested project to be active.

`scripts/setup-agora-mcp.sh` installs project listeners on loopback ports 18760
(Dock) and 18761 (Studio). Each requires its own random bearer stored only on
Agora in an owner-readable file under `.local/state/dock/mcp/`; that bearer is
never an argument or client configuration value. Unauthenticated requests return
401 and untrusted Host/Origin headers return 403. The owner connector configuration
contains `localAuthTokenFile`, a path rather than the credential. Owner probing
and proxying load it only for loopback HTTP, reject shared file permissions, and
replace upstream authorization without forwarding the registration bearer.

The owner advertises capabilities `dock.desktop.dock` and
`dock.desktop.studio` once enrolled and its daemon is running. The existing
cloud gateway resolves them at `/mcp/desktop.dock.desktop.PROJECT` using the
verified user's enrolled device route. No second gateway catalog or issuer is
created. `canvas-linux mcp-ready PROJECT` checks genuine owner application identity,
starts the configured owner daemon, then verifies real tools/list and
`desktop_status` through the cloud gateway, including the returned project ID.
It fails while enrollment is missing, a route is unavailable, or only a gateway
placeholder status tool exists.

Agora reaches the gateway's Batty loopback listener through
`dock-mcp-gateway-tunnel.service` while the public Shroud route remains
undeployed. Run `python3 scripts/setup-agora-mcp-gateway.py` on the Mac to
provision it. The script creates a dedicated Agora ED25519 identity, pins
Batty's host key, and limits that key to forwarding `127.0.0.1:8792`. FPL
credentials remain in the owner's OS keyring.

After enrollment and route verification, a manual equivalent is:

```sh
~/.local/bin/canvas-linux mcp-ready studio
~/.local/bin/fpl-desktop-bridge verify --connector dock-desktop-studio --local --desktop-read
~/.local/bin/fpl-desktop-bridge verify --connector desktop.dock.desktop.studio --desktop-read
~/.local/bin/fpl-desktop-bridge configure-client --client opencode \
  --desktop-connector desktop.dock.desktop.studio \
  --executable /home/ajmwagar/.local/bin/fpl-desktop-bridge
```

The owner configurator preserves unrelated model and client settings and writes
stdio launcher references. The launcher uses Agora's original Secret Service
bus even when OpenCode runs inside the isolated Sway session.

Live validation on Agora: OpenCode reports both project desktop servers
connected. Local and cloud-routed Dock and Studio connectors each completed
authenticated `tools/list` and `desktop_status`; all seven tools were present.
A protocol-level `desktop_recall` request under human ownership was refused,
confirming MCP cannot bypass takeover. Invalid local bearer and Host/Origin
boundaries remain 401 and 403 respectively.

## FPL Desktop app in the stream

Agora also runs the existing owner's iced app. Build the owner source, including
its assets, example config and vendored Coherence dependency, with:

```sh
cargo build --locked --features desktop --bins
```

Install the resulting pair of binaries on Agora using Canvas's installer:

```sh
sh scripts/setup-agora-fpl-desktop.sh /absolute/path/to/owner/target/debug
```

The installed `~/.local/bin/canvas-linux` must include `desktop-web-url` from this
revision. From a terminal inside the existing project Sway desktop, launch:

```sh
~/.local/bin/dock-fpl-desktop studio
```

The launcher preserves the Wayland display, uses the original user's GNOME
Secret Service bus, and routes only this app's `xdg-open` calls through
`canvas-linux desktop-web-url PROJECT HTTPS_URL`. The existing browser adapter
opens the selected project's persistent Firefox profile on Sway's private bus.
GNOME's browser defaults and activation environment are unchanged. General app
links require HTTPS without embedded credentials; `desktop-browser-url` retains
its stricter official pairing-URL contract. Both paths use the same browser
implementation. Neither path asserts browser authentication.

The GUI owns its child bridge daemon and stops it when closed. The launcher
refuses a second GUI and refuses to compete with an active
`fpl-desktop-bridge-dock.service`. To switch from the background service to the
GUI, stop that service explicitly first. While the GUI owns the route, use the
owner's `verify` CLI directly; `canvas-linux mcp-ready` starts the background
service and must not run alongside the GUI.

The installer does not change owner configuration or credentials. Normal app
startup detects existing AI clients and, after genuine sign-in, uses the owner's
existing client configurator. The matched sibling CLI wrapper retains keyring
access for those client entries. No model or inference request is involved in
installation or verification.

Live validation: the actual FPL Desktop window rendered through authenticated
1600×1000 Mac streaming, and a VNC mouse click switched it to Settings. Its
automatic sign-in opened another tab in the existing `dock-web-studio` Firefox
window. All five original project windows survived. The single-instance lock
and rejection of local-file browser handlers were checked separately.
See [the streamed FPL app](screenshots/fpl-desktop-agora.png).

Agora was enrolled by opening its public approval URL in the Mac
browser. FPL issued the credentials directly to Agora's polling owner process;
no Mac credential was copied. Fresh owner status verified `avery@fpl.dev`,
`session: authenticated`, and both credentials in the OS keyring. The application
session expires at `2026-09-11T22:27:08.540849316Z`, inherited from the approving
SSO session. The desktop browser handoff then created a separate derived browser
session through explicit browser approval and an exact loopback callback. Its
public URL contains no durable credential, is single-use, expires after 120
seconds, and remains bounded by the parent desktop session. Revocation, expiry,
scope reduction, or device rotation invalidates the derived browser session.
