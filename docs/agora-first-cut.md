# Agora One first cut

Verified over existing SSH/Tailscale on 2026-09-06. `agora-one` resolves to
`100.80.85.86`; SSH user is `ajmwagar`. Ubuntu 26.04, GNOME Wayland, NVIDIA
RTX 3060 (driver 595.84), systemd 259. A physical Dell display is connected at
1920×1200. The existing desktop and its lock/idle settings are preserved.

## Rust process adapter

`crates/canvas-linux` is installed at `~/.local/bin/canvas-linux` on Agora One.
It accepts argument vectors and emits JSON with `schema_version: 1`, `ok`, and
either `result` or `error`; errors also exit nonzero. Transport is existing SSH,
not a new listening service.

```sh
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux capabilities
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux run demo /tmp -- /usr/bin/sleep 300
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux status demo
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux pause demo
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux resume demo
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux logs demo
```

Project identifiers contain 1–64 lowercase ASCII letters, digits or hyphens.
They map to `dock-project-PROJECT.service` in the current user's systemd manager.
The adapter checks its unit description marker before managing an existing
unit. A duplicate launch fails rather than replacing a process. Directory must
exist and be absolute. Arguments go directly to `systemd-run`, without shell
evaluation inside the adapter. SSH callers must still quote remote arguments
properly; the examples above intentionally use simple arguments.

Pause freezes that unit's cgroup and verifies `FreezerState=frozen`; resume
verifies `running`. Desktop applications, remote EdgeRunner sessions, and
other project processes remain outside that scope. Freeze is not checkpointing:
state remains in memory, network connections may time out, and a reboot loses
the running process. Fast-exiting programs can disappear before a subsequent
status query because these are collected transient units; launch acceptance is
not a claim that a program remains running or completed successfully.

This first cut supports **noninteractive programs**, not interactive terminal
sessions or prompting Codex/Claude/OpenCode. The SSH account is the trust
boundary; the ownership marker is protection from mistakes, not isolation
against other processes running as the same user. There is no backend agent
lease or human-takeover enforcement yet. Capabilities explicitly report no
desktop window control or computer use. Interactive session integration is
tracked in `dock-96j.11`.

Manual stop/recovery uses ordinary systemd tools:

```sh
ssh -o BatchMode=yes agora-one systemctl --user stop dock-project-demo.service
```

Validation: four Rust tests pass on Mac and Linux. A fifth Linux integration
test launches a real heartbeat process, confirms that its output stops while
frozen and grows after thaw, then stops the unit and removes its temporary
files. Run on a Linux host with a systemd user session:

```sh
cargo test -p canvas-linux -- --include-ignored
```

## Streaming installed

Installed the official Sunshine `v2026.516.143833` Ubuntu 26.04 amd64 package
and its two small apt dependencies. Also installed the Ubuntu Rust/Cargo 1.93
toolchain to build the adapter natively. Sunshine uses the packaged user service
`app-dev.lizardbyte.app.Sunshine.service` (alias `sunshine.service`), enabled for
the graphical session. The native package grants the binary the documented
capture capabilities and installs uinput rules.

Configuration at `~/.config/sunshine/sunshine.conf`:

```ini
sunshine_name = Agora One
bind_address = 100.80.85.86
address_family = ipv4
upnp = disabled
origin_web_ui_allowed = wan
lan_encryption_mode = 2
wan_encryption_mode = 2
capture = kms
encoder = nvenc
```

`wan` permits the Tailscale peer to reach administration; the bind address
restricts listeners to the Tailscale interface. The administrator account is
`dock`; a generated password is stored only on Agora One in
`~/.config/sunshine/dock-admin-password` with mode 0600. Retrieve it over SSH
when pairing, rather than storing it in project configuration. Administration
is at `https://100.80.85.86:47990` with Sunshine's self-signed certificate.
Streaming listeners were verified on the Tailscale address, not all interfaces.
UPnP is off and Sunshine encryption is mandatory in both network modes.

Initially KMS found no enabled monitor because GNOME had powered the display
down. Waking its output through the existing Mutter D-Bus interface fixed
capture without unlocking GNOME or changing its idle settings:

```sh
ssh -o BatchMode=yes agora-one "gdbus call --session --dest org.gnome.Mutter.DisplayConfig --object-path /org/gnome/Mutter/DisplayConfig --method org.freedesktop.DBus.Properties.Set org.gnome.Mutter.DisplayConfig PowerSaveMode '<0>'"
ssh -o BatchMode=yes agora-one systemctl --user restart sunshine
```

For a bounded preparation window, the Rust adapter now owns the wake sequence
and a temporary GNOME idle inhibitor:

```sh
ssh -o BatchMode=yes agora-one .local/bin/canvas-linux prepare-stream 600
```

The duration must be 30–900 seconds. A transient user service named
`dock-stream-preparation.service` automatically releases the inhibitor on
timeout; an overlapping preparation is rejected. To release early, use
`systemctl --user stop dock-stream-preparation.service`. This temporarily
inhibits GNOME idle behavior while preserving the current locked state; it
does not permanently alter lock/idle settings. It is a preparation lease, not
yet an inhibitor automatically tied to stream connect/disconnect. The command
does not restart Sunshine or claim to verify stream readiness. Live checks
confirmed the inhibitor registered and `LockedHint=yes` remained set.

Moonlight pairing is confirmed: Sunshine's client API lists enabled client
`Avery Mac Studio Dock`. The initial streaming attempt failed with error 503
after the display slept again; the bounded preparation mechanism addresses
that immediate test condition. The retry succeeded: Sunshine logged
`Video encryption enabled`, one active session, and `CLIENT CONNECTED` at
03:38:50 PDT, using HEVC NVENC and Opus audio. The Mac Moonlight log at
`/tmp/Moonlight-1788691122.log` reported a 1920×1080 60fps stream, the first
video packet, a decoded frame, and first audio at 400ms. GNOME remained locked.
This verifies encrypted video delivery and decoding, plus audio transport;
human interaction, reconnect after idle, and sustained performance remain
unverified. The live validation's temporary `dock-stream-check.service`
inhibitor expires at approximately 03:47:24 PDT. The stream was left connected
for the user; there is no permanent keep-awake setting.

Sunshine then reported KMS screencasting and working H.264/HEVC NVENC encoders.
AV1 is unsupported on the RTX 3060 and is not required. Real Mac
input checks, reconnect behavior, and display wake policy are tracked in
`dock-96j.10`. `sunshine_running` in the adapter means the service is active;
it does not imply capture readiness or a paired client.

Manual recovery: `systemctl --user restart sunshine`; logs are at
`~/.config/sunshine/sunshine.log`. To stop streaming and its autostart, use
`systemctl --user disable --now app-dev.lizardbyte.app.Sunshine.service`.

References: [official release](https://github.com/LizardByte/Sunshine/releases/tag/v2026.516.143833),
[official configuration](https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2configuration.html).
