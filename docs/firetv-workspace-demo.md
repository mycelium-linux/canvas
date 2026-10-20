# Shared mock workspace on Fire TV

The `workspace-demo` Android feature mounts `canvas-workspace-ui::shell::Shell`
through a native GLES3 adapter. It runs the same Rust mail, work and calendar
components as the desktop preview. No screenshot streaming, browser, provider
credentials or Google access is involved.

The portable definition is `config/deploy/firetv/workspace-demo.json`. Its
`external_surface` source `canvas-workspace-demo` names the built-in Android
producer. This is one explicitly registered local producer, not general Android
support for external native-handle registration. Other hosts must advertise
support before presenting it.

Java owns EGL lifetime, remote/keyboard/pointer delivery and app-private
SharedPreferences. The Rust adapter owns one painter and shell per GL-thread
context generation. Input queues, geometry, input sizes and checkpoints are
bounded. Errors appear as an explicit error panel; invalid saved state is not
replaced. GLES paints directly to the surface, with no runtime CPU readback.
The offscreen screenshot tool is isolated in `canvas-workspace-review`.

D-pad Up/Down selects conversations. OK opens the selected conversation or
activates a focused control. Left/Right moves control focus. Fast-forward
archives, rewind undoes, play/pause toggles bulk selection, and Channel/Page
Up/Down scrolls the reader. Back closes local overlays and releases control
focus; Menu returns to the Canvas workspace selector. A hardware keyboard and
pointer can use the shared UI. This adapter does not implement Android IME
composition, clipboard or accessibility text editing; remote triage and the
rendered content are the initial qualification surface.

Checkpoints retain mock triage, snooze deadlines and draft buffers, once per
second and before pause/detach. Save completes before the corresponding GL
context is destroyed, with a bounded wait; failures retain the prior checkpoint
and are reported. Android process recreation loads the last checkpoint. The
same schema validation is used on native and web preview hosts.

Build with the current canonical Android source and NDK r26d for
`armv7-linux-androideabi`, API 24, with `--features workspace-demo`. Compile Java
with JDK 17, UTF-8 and Android 33; package all current assets and the existing
manifest. Reuse the installed development signing identity. Preserve the
installed APK for rollback, use `adb install -r`, and never clear app data or
replace the full catalog. Import the single document with the current owner
revision, then explicitly activate `mail-workspace-demo`. Verify actual pixels,
remote input, checkpoint/recreation and retention of previous projects.

Android validation uses the current ARMv7 target and JDK 17 Java compilation.
The shared UI suite includes clock/checkpoint stability so an idle view does not
rewrite flash each second. Rust 1.99 currently flags an explicitly const
`thread_local` initializer through macro expansion; the adapter documents that
narrow lint exception. Full Android Clippy also encounters existing
`collapsible_if` warnings in `native_control.rs`; scoped checks can allow that
baseline lint while keeping all other warnings denied. The shared UI's own
strict Clippy check needs no exception.

## Hardware qualification, 2026-10-19

APK 64 (`0.1.63-workspace-egui`) was installed in place on the registered
`firetv` endpoint, AFTTIFF43, API 28 / ARMv7. APK v2/v3 signature verification
passed and its certificate matched APK 63. All 31 prior projects survived;
`mail-workspace-demo` was added and activated. The 1920x1080 logical display
override was preserved.

Actual device screenshots verified Mail, Work and Calendar. Physical remote-key
injection verified conversation selection, archive/undo, two-row bulk
archive/undo, and persisted selection/star state across process recreation.
The star and archive fixture counts were restored afterward. Home/foreground
lifecycle resumed the view. An idle checkpoint's filesystem mtime stayed
unchanged across three seconds of repainting. The final screen was returned to
Mail. A device-specific ADB reconnect recovered one temporary transport drop;
no TV reboot, data clearing or global ADB-server reset was used.

The Android build was based on Neo's canonical `9b60735`, in isolated worktree
`/Users/ajmwagar/Documents/src/fpl/dock-workspace-demo`, branch
`feat/firetv-workspace-demo`. The older desktop preview checkout is not the
canonical Android deployment base. APK 63 and the pre-upgrade owner observation
remain in Neo's private `~/.local/share/canvas-android/artifacts/workspace-demo-rollback/`.

Validation: 18 shared UI tests, 9 Coherence tests, strict shared-UI Clippy,
WebGPU type-check, ARMv7 type-check, Java compilation, signed APK validation,
and Android Clippy with the documented existing `collapsible_if` baseline
exception. These checks do not qualify a real Google account connection or
Android IME composition.
