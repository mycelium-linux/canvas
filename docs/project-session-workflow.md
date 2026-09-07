# Return to a project, its desktop, and its conversation

Run the Rust iced shell against an existing project:

```sh
cargo run -p dock-mac -- --connect-project dock
```

Selecting Dock or Studio applies its Coherence palette, recalls attached Mac
windows, selects the corresponding independent Linux workspace, and reopens that
project’s persistent OpenCode terminal and browser. The Linux service owns the
project directory and conversation ID. Recalling a project does not create a new
conversation, and leaving it does not terminate its apps or agent session.
Project navigation is serialized while recall is pending, so rapid switching
cannot leave the remote desktop on a different project from the selected one.
Human takeover remains available during recall.

Dock is a small project switcher, with the desktop itself in the live viewer.
Press **Ctrl+Option+Space** anywhere to summon it. In Dock, use **Up/Down then
Enter**, click a project name, or use **Command+1 / Command+2**. Selecting a
project opens or reuses the existing TigerVNC connection, then minimizes the
companion. The registered global shortcut uses public Carbon APIs and needs no
Accessibility grant. If registration fails, Dock displays the error; use
Command-Tab to return to Dock.

Type one task in the prompt line and press **Enter**. Dock grants agent control
for that submission and sends it to the existing conversation. It returns control
after observing a new completed assistant message and idle status. The latest
assistant reply and progress appear without session IDs or tool JSON. **Escape**
in Dock interrupts the managed request and takes human control. A prompt can say
“continue” to resume the same history. Merely viewing the desktop or summoning
Dock does not take control. Escape is local to Dock, not a global interrupt key.

Automatic grants carry the ownership epoch observed before submission. A human
takeover increments that epoch, so an older delayed grant cannot undo it. Failed
submissions attempt immediate human takeover; cleanup failures remain visible.
Background projects continue polling until their submitted task finishes.

TigerVNC uses the independent Sway workspace through the private SSH tunnel at
`127.0.0.1:15901`. Its server certificate is retrieved through authenticated SSH;
credentials stay in the viewer’s environment, never argv. Project recall reuses
an existing viewer process for that endpoint. Closing Dock does not terminate the
persistent Linux workspace. Sunshine/Moonlight for ordinary Ubuntu remains
separate. The viewer is live video; there is no snapshot dashboard in the default
companion.

For window bindings, stream-display recovery, detailed session diagnostics and
explicit screenshot/input controls, launch **`dock-mac --diagnostics`**. These
remain available without cluttering the normal project workflow. An optional
`--model PROVIDER/MODEL` overrides OpenCode’s existing default model; there is no
model selector in the everyday UI.

## Agent and manual access

The GUI and Rust command line use the same bounded SSH transport and versioned
responses:

```sh
# Read the existing conversation and current desktop ownership.
cargo run -p dock-mac -- remote dock session-status
cargo run -p dock-mac -- remote dock desktop-status

# Explicitly allow work, then submit to the same registered conversation.
cargo run -p dock-mac -- remote dock desktop-control agent
printf '%s' 'Inspect the project and report what you find.' | \
  cargo run -p dock-mac -- remote dock session-prompt

# Interrupt without deleting context, or take control of both session and input.
cargo run -p dock-mac -- remote dock session-abort
cargo run -p dock-mac -- remote dock desktop-control human
```

Prompts travel on stdin, not in executable shell text. Other remote arguments are
quoted as individual POSIX shell arguments. Requests have bounded output and
timeouts; a timeout never automatically resubmits a prompt. Query status before
retrying a submission with an uncertain acknowledgement.

Known initial directories are owned by the backend: Dock uses
`/home/ajmwagar/src/dock`, and Studio uses `/home/ajmwagar/src/dock-studio`.
Additional projects must first register a session through `session-start` with an
explicit directory. The GUI consumes the shared project catalog and does not
maintain its own directory mapping or conversation history store.

Mac window operations continue to use the separately installed Dock Control.app.
Its Accessibility grant is needed for Mac focus and tiling, but not for the
independent Linux viewer, session APIs, captures, or remote input.

## Authenticated Edgerunner owner reads

The credential-owner path is available without giving Dock an app token:

```sh
dock-mac edgerunner owner-list mac
dock-mac edgerunner owner-status mac EXISTING_SESSION_ID
dock-mac edgerunner readiness mac EXISTING_SESSION_ID
```

Use `agora` instead of `mac` to use that machine's FPL Desktop Bridge credentials.
The owner CLI must be installed and signed in, and its `desktop.edgerunnerUrl`
configuration must name the authorized HTTPS Edgerunner origin. Dock delegates to
`fpl-desktop-bridge owner-session list|status|readiness`; it never receives the
bearer or builds private VM routes. Session acknowledgements must match the
requested ID. Existing model metadata is preserved without overrides.

The owner service must explicitly assign the chosen existing session to the
verified account in its operator registry. Missing mappings fail closed. This
source integration does not imply that registry deployment or a real signed-in
session probe has already happened. Provider configuration and model listing
are reported separately from inference authorization; readiness performs no
inference or tool execution.
