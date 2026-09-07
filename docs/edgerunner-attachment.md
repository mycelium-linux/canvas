# Attach to the existing session owner

Edgerunner owns coding sessions, models, credentials and runtime lifecycle. Dock
owns the project desktop and its presentation. The local OpenCode services on
Agora were integration proofs; Big Pickle was a test model, not the deployment
policy for the user's work.

The live Edgerunner is on Batty. The current owner source is
`/Users/ajmwagar/src/edgerunner`; the checkout on Agora is older and is not the
service to deploy. Dock's thin read client uses the existing authenticated SSH
route to Batty and the owner's internal API. It does not provision runtimes,
replace model settings, or copy service credentials.

```sh
cargo run -p dock-mac -- edgerunner list
cargo run -p dock-mac -- edgerunner status SESSION_ID
cargo run -p dock-mac -- edgerunner bind PROJECT SESSION_ID
cargo run -p dock-mac -- edgerunner bound-status PROJECT
```

`bind` validates the live owner session and writes only a private local reference.
It does not start, claim, prompt, or attach a terminal to that session. No binding
has been inferred for Dock or Studio: neither has a matching existing owner
session. A bound project cannot silently fall back to the local proof-session
adapter.

The verified read API lists 71 existing sessions. Owner-managed models are
reported as metadata; Dock does not override them. Connection configuration can
be supplied in `~/.config/dock/edgerunner.json` with `ssh_host` and `base_url`.
The defaults refer to the existing Batty deployment.

Interactive attachment remains tracked in `dock-96j.11`. It needs the selected
existing project and the owner's authenticated attachment route. Edgerunner
exposes message/event/lifecycle APIs, but its raw OpenCode service address is
currently an internal backend detail, and the password endpoint requires normal
authenticated identity. Dock must consume an owner-issued attachment descriptor
or proxy instead of reconstructing Shroud port mappings or extracting runtime
secrets. Read-only discovery is verified; full owner-backed terminal/control
attachment is not yet implemented.
