# Persistent project coding sessions

Agora One runs OpenCode's supported HTTP backend independently of its terminal.
Dock stores the selected session ID per project. A persistent tmux terminal
attaches to that exact ID; the iced shell submits prompts and reads progress from
the same backend. Switching projects does not restart the conversation or its
processes. Edge Runner remains the intended long-term owner of session hosting;
this adapter is a narrow local backend integration.

The initial backend directories are `~/src/dock` and `~/src/dock-studio`. A project
can first be registered explicitly using `session-start PROJECT DIRECTORY`.
Before first registration, `~/.config/dock/project-directories.json` can override
the defaults with a JSON map such as `{"dock":"/home/ajmwagar/src/dock"}`.
Once registered, the persisted canonical directory takes precedence.
The directory is canonicalized and a subsequent request for the same project
with a different directory is rejected, preserving session identity.

```sh
~/.local/bin/dock-linux session-ensure dock
~/.local/bin/dock-linux session-attach dock
~/.local/bin/dock-linux session-status dock
~/.local/bin/dock-linux desktop-control dock agent
printf '%s' 'Read Cargo.toml and describe the workspace.' | \
  ~/.local/bin/dock-linux session-prompt dock opencode/big-pickle
~/.local/bin/dock-linux session-status dock
~/.local/bin/dock-linux session-abort dock
printf '%s' 'Continue after inspecting the current files.' | \
  ~/.local/bin/dock-linux session-resume dock opencode/big-pickle
```

Prompts arrive on standard input, not shell arguments. `session-resume` sends a
new continuation prompt into the same conversation; it does not replay a killed
OS process. An empty resume prompt uses an explicit instruction to inspect
current state before continuing. `session-status` returns the session ID, status,
20 recent messages and tool parts, and whether its tmux terminal is alive. An
accepted asynchronous request is not a completed inference: poll status and
inspect message errors and tool results.

The backend checks Dock's shared project control latch while accepting a prompt.
The default is human control. Human takeover revokes that latch before requesting
OpenCode's supported abort operation. The local account can still interact with
OpenCode directly; the latch governs Dock's orchestration interface, not a
sandbox around the user account. OpenCode owns model permissions, conversation
persistence, tool execution, and cancellation semantics.

Each project backend is a systemd user service named
`dock-opencode-PROJECT.service`, with its working directory fixed at registration.
It listens only on `127.0.0.1` on a recorded per-project port; a generated local
HTTP password stays in a private 0600 environment file, never a CLI argument.
State lives under `~/.local/state/dock/opencode` (0700). Dock does not expose this
HTTP listener on Tailscale or the LAN. Remote clients invoke the narrow Rust CLI
through SSH. Provider credentials are managed by OpenCode, not copied into Dock.

The terminal uses a dedicated tmux server (`tmux -L dock-opencode`) and a session
named `dock-PROJECT`. `session-attach` opens it inside Dock's isolated Sway desktop.
Closing a terminal window leaves the backend and tmux session alive. Manual
recovery is possible with `tmux -L dock-opencode attach -t dock-dock`.
Backend logs are available with:

```sh
journalctl --user -u dock-opencode-dock.service --no-pager --lines=50
```

OpenCode was installed from its [official installer](https://opencode.ai/docs/).
The adapter uses the documented [server API](https://opencode.ai/docs/server/)
and [attach command](https://opencode.ai/docs/cli/). Initial installation on Agora
One is version 1.18.29. No provider credentials existed at installation; model
availability and successful inference must be verified separately from backend
health. The distribution advertises free models usable for an initial live proof.

## Verified live on Agora One, 2026-09-06

Two separately registered projects survived switching and repeated ensure calls:

| Project | OpenCode session | Loopback port | Visible terminal |
| --- | --- | --- | --- |
| dock | `ses_f898de9eeffepmtAaSgITDIKA7` | 38253 | `dock-session-dock` |
| studio | `ses_f8989986affeM7eH4GLvt5M3lf` | 42277 | `dock-session-studio` |

These are observed runtime identities, not required configuration. Both actual
OpenCode TUI processes were checked to attach to their recorded session IDs.
Both listeners bind to 127.0.0.1 only and return HTTP 401 without local service
authentication.

With `opencode/big-pickle`, the Dock session used completed **read** and **edit**
tool calls to update the browser fixture heading and add a working Reset button.
The changed file is `examples/workspace/index.html`; the Reset button clears its
interaction counter and restores the initial output text.

A subsequent authorized cancellation test entered `busy` with an actual Bash
tool running `sleep 45`. Human takeover returned an acknowledged OpenCode abort.
The same session became `idle`, recorded `MessageAbortedError`, and its tool
output reported user cancellation without executing the post-sleep finish marker.
A new Dock prompt was rejected while human ownership remained in effect. After
an explicit grant back to the agent, `session-resume` retained the same ID and
conversation, correctly recalled both prior edits, and completed without rerunning
the canceled command. Desktop input refusal and visual browser verification are
recorded separately in the desktop runbook.
