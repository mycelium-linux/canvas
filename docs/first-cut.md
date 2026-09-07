# Dock: shared project workspaces

This consolidates the user's corrections to the original build note. The product
is a workspace that a person and an agent can both operate, spanning Agora One's
Ubuntu GPU desktop, local Mac windows, and existing Edgerunner/OpenCode sessions.

Alfalfa owns project context, shared coordination and bulletin-board resources.
Dock consumes that context and presents the workspace. Edgerunner/OpenCode owns
agent conversations and model selection; fpl-auth and the existing desktop bridge
own identity, enrollment and credential handling. Dock is not an agent harness.
The [startup integration](project-startup.md) distinguishes working adapters from
the remaining authenticated session and tool integration.

Selecting a project brings its relevant resources into reach. The workspace's
Coherence palette provides visual context without a large project-name header.
Dock and Studio have independent running coding conversations and desktop windows
on Agora One; the catalog provides their project identities.

The implementation uses Rust and an iced shell. Existing Sway/dotfiles are
interaction references. Ubuntu GNOME stays in place. The independent project
desktop runs headless Sway, with wayvnc/TigerVNC over an authenticated SSH tunnel.
Sunshine/Moonlight still serves the original GNOME session. Each stream is one remote surface,
not individual remote windows exposed through a compositing API.

## Boundaries

`dock-core` holds local project/resource bindings, Coherence token selection, and local
workspace control policy. `dock-mac` owns the iced presentation and public Mac
window orchestration. The independently installed `dock-control` helper owns
native Accessibility operations; ordinary GUI rebuilds leave its permission
identity untouched. `dock-linux` owns explicitly launched systemd user
processes, the independent Sway desktop, and adapters to OpenCode's supported
session API. The earlier nested Hyprland setup remains exploratory. Streaming
remains owned by the existing transport implementations. Alfalfa, Edgerunner,
Bifrost, identity and compute orchestration remain external owners. Their
interfaces are integrated without duplicating their lifecycles in Dock.

Native tokens are an exact vendored snapshot of Coherence's existing native
crate. Project themes select upstream light/dark mode and a semantic accent;
they do not create a parallel color system. The shell consumes upstream spacing
and dimensions as well as colors. The same source generates Sway/Hyprland borders,
gaps and per-project foot palettes. Workspace recall applies the
project's compositor theme; unrelated applications and GNOME keep their styling.

On Mac, a reviewed display reservation evacuates local windows to selected
remaining displays before placing the project VNC viewer fullscreen on the
reserved display; the older Moonlight adapter remains available in the library.
A durable journal records original frames before each move and supports rollback
and crash recovery. Actual fullscreen streaming validation requires native
Accessibility access. The independent headless project desktop does not require
unlocking GNOME; the original Sunshine/GNOME stream has its own lock state.

## Control and lifecycle

Project selection changes workspace visibility and preserves background context.
It does not stop agents, freeze the desktop or suspend a machine.

The Linux adapter's explicit pause/resume freezes/thaws the cgroup of a process
launched by Dock. This covers that process's descendants, not external sessions.
The state model records a pause only after a successful adapter acknowledgement.
Separately, OpenCode owns persistent coding conversations. Dock prompts,
observes, interrupts and continues the recorded session through its API while a
tmux-backed terminal attaches to that same session. These controls do not imply
equivalent integration with Codex, Claude or Edge Runner.

The human can reserve local workspace control. Agent-facing Mac operations must
respect that reservation. This is cooperative control among local clients,
not authentication or a security sandbox: the CLI actor flag is not proof of
identity, and direct SSH/OS access can act outside Dock. Linux takeover persists
the human latch under the same lock as agent input and prompt submission, then
requests OpenCode cancellation. It reports an abort failure explicitly. This
does not interrupt coding agents managed outside that registered OpenCode session.

## Initial validation

The useful experiment is a project with local Mac windows, the Agora One desktop
stream, and a managed process whose state can be inspected, paused and resumed.
Window bindings, project selection and Coherence appearance should survive
ordinary switching without fighting direct human keyboard/mouse use.

Live capabilities and limitations are recorded in the platform runbooks. Beads
epic `dock-96j` owns implementation work and followups; `dock-2kf` retains future
architectural candidates. A custom compositor, bespoke crypto/streaming protocol,
and inference dependency are not required for this experiment.
