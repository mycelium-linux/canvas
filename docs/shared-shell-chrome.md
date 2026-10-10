# Shared shell chrome

`fpl-gfx::chrome` owns control face geometry, semantic color roles, interaction
hierarchy and standard/quiet appearance. Coherence supplies actual colors and
shape tokens. Native shells retain labels, layout, input, accessibility and
actions; no platform event handling belongs in GFX.

Desktop Neo and Studio use the same iced adapter in
`canvas-widgets::live::app_chrome`. FireTV's `ShellChrome` is a cached native
drawable visitor for Rust-generated GFX records, not a second styling engine.
Both use quiet idle navigation, an opaque themed bar, primary outlined
selection/focus, a recessed pressed face and subordinate disabled ink. Remote
focus remains more prominent than persistent selection. Press and focus do not
cause layout or video surface changes.

The JNI frame contract is ten floats: x, y, width, height, radius, outline
width, fill role, outline role, label role and face opacity. Java and Rust must
ship together: a native-library-only upgrade is not compatible with this frame
contract or its quiet-appearance argument. Rebuild the complete APK rather than
replacing only its native library. Desktop Lua composition stays editable
without rebuilding the renderer; the analog clock and workspace definitions
are unchanged.

Validate with GFX `cargo test --no-default-features chrome::tests`, desktop
`cargo test -p canvas-widgets --features live --lib live::app_chrome::tests`, Android native
chrome tests, and a Java compile against the supported Android SDK. Live
acceptance requires desktop pointer/keyboard and TV D-pad checks for selected,
focused, pressed and disabled controls, preserving the selected workspace.

This slice does not consolidate app-specific document toolbars or every legacy
widget action style, nor does it claim identical pixel density across desktop
and a distant TV. Those are separate presentation concerns, not new platform
renderers.
