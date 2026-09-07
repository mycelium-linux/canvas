# Agora One: actual tiling desktop

This records the earlier nested Hyprland experiment. The working independent
Sway/wayvnc desktop and validated human/agent input are documented in
[agora-desktop.md](agora-desktop.md).

Hyprland 0.53.3 is installed from Ubuntu 26.04's native package repository,
alongside foot 1.25 and grim. Agora One's NVIDIA 595.84 and Xwayland 24.1.10
meet the modern versions described by Hyprland's NVIDIA guidance. No display
manager was replaced and GNOME was not terminated.

There is a live **nested Hyprland compositor**, not a mock window layout.
It runs in `dock-hyprland.service`, with a private D-Bus session and private
runtime directory `/run/user/1000/dock-hyprland`. The parent GNOME Wayland
socket is addressed by its absolute path. This separation matters: early
testing found nested helpers changing the shared systemd activation
environment. The isolated launch fixes that; the global environment was
verified to remain `WAYLAND_DISPLAY=wayland-0`,
`XDG_CURRENT_DESKTOP=ubuntu:GNOME`, and `DISPLAY=:0`.

The first cut uses the packaged compositor directly inside a transient user
service, instead of its normal session launcher, deliberately keeping session
ownership with GNOME. A five-second stop timeout bounds compositor cleanup.
The service is not enabled at login. The package also installs ordinary
Hyprland login-session entries for a later dedicated desktop decision.

## Use it from the Mac

Keep using the original paired Agora One desktop in Moonlight. Sunshine
remains on **KMS capture of the host desktop**. Unlock GNOME normally, then
focus its Hyprland window; maximize it with GNOME's normal window controls.
That makes the normal seat input route reach the nested compositor.

GNOME was still locked during this implementation. We did not bypass its lock.
The nested compositor's IPC and clients work, but GNOME suspends rendering of
its background window while locked. A direct Sunshine `capture=wlr` probe
initialized NVENC on the nested output, but did not prove interactive frames.
Sunshine's uinput events also target the host seat. For those reasons the probe
was temporary and the primary stream was **not** switched to direct nested
capture. A headless-output experiment was removed. No new probe listener is
left running.

Human input, visual inspection after normal unlock, and sustained streamed
tiling remain to be verified. Do not interpret the existing successful GNOME
stream or compositor IPC checks as proof of those steps.

Inside the nested window:

| Shortcut | Action |
| --- | --- |
| Alt+1 / Alt+2 | Recall Dock / Studio, including its Coherence theme |
| Alt+Return | Open a terminal using the active project's foot theme |
| Alt+H/J/K/L | Focus left/down/up/right |
| Alt+Shift+H/J/K/L | Move the focused tile |
| Alt+F | Toggle fullscreen within Hyprland |
| Alt+Shift+1 / Alt+Shift+2 | Move a window to Dock / Studio without switching |
| Alt+Q | Close focused window |

Alt avoids GNOME's Super bindings; GNOME may still intercept some shortcuts
until the nested window is focused. The full native session can make its own
binding choices later.

## Rust control contract

```sh
ssh -o BatchMode=yes agora-one .local/bin/dock-linux hypr-start
ssh -o BatchMode=yes agora-one .local/bin/dock-linux hypr-status
ssh -o BatchMode=yes agora-one .local/bin/dock-linux hypr-project studio
ssh -o BatchMode=yes agora-one .local/bin/dock-linux hypr-open studio /tmp
ssh -o BatchMode=yes agora-one .local/bin/dock-linux hypr-focus 0xCLIENT_ADDRESS
```

`hypr-start` verifies config, starts or registers the owned service, and selects
Dock. Its registry at `/run/user/1000/dock-hyprland.instance` selects one
explicit compositor. Startup validates the process belongs to that service's
cgroup; it never picks an unrelated Hyprland instance. Other commands use the
private runtime. GUI and agent clients receive the same JSON envelope as the
process adapter, with nonzero exit status on failure.

`hypr-project ID` checks its generated theme exists, loads it, checks parser
errors, selects the workspace, and verifies its resulting name. Success returns
`result.active_workspace.name`, `result.theme_applied=true`, and
`result.theme_source`. Hyprland may exit zero while rejecting a request, so the
adapter also checks textual `ok` acknowledgements. Version 0.53's clean
`configerrors` response can be `[""]`; that empty entry is not an error.

`hypr-open` starts a real foot client in the named project workspace with
its project theme. Launch acceptance is reported separately from the client
appearing; inspect `hypr-status` to verify it. Directory arguments are quoted
for Hyprland's command shell. `hypr-focus` only accepts an observed client
address and verifies focus afterward. `capabilities` reports window control
only when the registered compositor responds, scoped to this Dock instance.
This does not yet implement generic screenshot/input tools, coding-agent
prompting, or backend human-control leases.

## Configuration and validation

`config/linux/hyprland.conf` owns compositor behavior. Parent-generated
Coherence artifacts are installed as:

```text
~/.config/dock/hypr/hyprland.conf
~/.config/dock/hypr/coherence.conf
~/.config/dock/hypr/foot.ini
~/.config/dock/hypr/foot-dock.ini
~/.config/dock/hypr/foot-studio.ini
~/.config/dock/hypr/themes/dock.conf
~/.config/dock/hypr/themes/studio.conf
```

Regenerate with `cargo run -p dock-core --bin dockctl -- export-desktop config/linux`
(see the repository README for the current CLI package command). Do not edit
generated palettes separately. Theme files include actual border, gap and
rounding assignments, so sourcing them applies the new project appearance.
Existing terminals retain their own project's palette when another project
becomes active.

Live observations on Agora One:

| Workspace | Mapped clients | Tile geometry | Active border readback |
| --- | --- | --- | --- |
| Dock | Two foot terminals, neither floating | `(10,10) 620×700`, `(650,10) 620×700` | `ff83cbdc 0deg` |
| Studio | One foot terminal, not floating | `(10,10) 1260×700` | `ff2449d8 0deg` |

The output is 1280×720. Switching Studio→Dock read back both correct workspace
names and distinct live border values with `hyprctl -j getoption
general:col.active_border`. Client focus was also observed and acknowledged.
`Hyprland --verify-config` passed; foot clients mapped using the generated
configuration. Six native Linux tests passed, including the earlier live
process freeze/thaw heartbeat test and shell-argument quoting. Local Clippy
passed. No screenshot is presented as validated appearance while GNOME keeps
the nested surface suspended.

To stop only this nested desktop, first save its work, then run:

```sh
ssh -o BatchMode=yes agora-one systemctl --user stop dock-hyprland
```

GNOME and Sunshine remain available. Restart with `hypr-start`; transient
terminal windows are not persisted across compositor shutdown. Selecting
another project preserves its running windows; pausing a project process is
still the separate systemd operation documented in `agora-first-cut.md`.

References: [Hyprland NVIDIA guidance](https://wiki.hypr.land/Nvidia/),
[hyprctl interface](https://wiki.hypr.land/0.53.0/Configuring/Using-hyprctl/).
