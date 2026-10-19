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

## TV audio control

Video surfaces keep a compact 24×24dp speaker action using Android's native
muted/unmuted icons, not a font glyph. These are remote/D-pad controls, not
touch targets. The focus outline remains visible; the accessible action is
still `Mute` / `Unmute`.
One press changes only that decoder's gain; it never touches system volume or
other tiles. Repeated stream titles are removed from the visible label, but
the accessibility description retains the exact source and current mute state.
`Unmute` means this stream is muted, not that playback or audible signal was
verified. The ordinary main stream and additional independent streams share
the same label contract. JVM-only `AudioChromeTest` covers that contract.

## Compact shell hierarchy

GFX owns the shared 44-logical-pixel desktop shell row height. The Android Rust
adapter derives its TV profile (`ceil(height / 3)`, currently 15dp), with 10sp
labels and 6dp outer padding, and exports it through JNI. Desktop stays 44px.
Android's default row is `fungOS / selected workspace / ↓ Browse / current page / clock`, with a
single workspace action revealing the retained grouped navigation and page
choices. Expanded choices stay in the same horizontal row; they never become
a second stacked toolbar or replace workspace content. Selection collapses the
menu, while MENU focuses the first actual control rather than the brand label.
At density 2 on the Omni's 1920×1080 logical display, the bar falls from 88px
to 30px. This does not change Android's display size or any other application.
The brand uses the theme's green success ink. Page controls and the Library
launcher remain available in the expanded navigation.

Down or Select on the workspace selector opens and focuses the first choice.
Left/Right traverses the visible enabled controls and clamps at either end.
Down on a category enters its first workspace; Up returns to the category.
Down from a workspace choice closes the menu and enters interactive content
(or the source audio control for video-only workspaces). Up from source audio
returns to the selector. MENU restores selector focus; Back collapses the menu
before leaving the Activity, without superseding Library or Channels handling.
Content retains its own PDF panning and signal-widget arrow handling. Remote
navigation does not implicitly grant keyboard control of a Surf stream.

From a focused main video, Up returns directly to the selector and Down enters
its audio control. The horizontal scroller itself is not a focus stop.

### Omni verification — 2026-10-19

Installed development APK version 57 (`0.1.56-tv-compact-navigation`) in place,
using the existing signing certificate and without clearing app data. The
selected FPL venture deck (document revision 3) and all 31 workspaces survived.
ADB screenshots and accessibility bounds confirmed the 30px bar and 48×48px
speaker target on the 1920×1080 logical display. Verified selector Down,
Left clamping, category Right/Down, child Up, Back collapse, source-audio Down
and Up-to-selector on the device. Decoder connection 1607 remained unchanged
through navigation; its initial 85-frame and later 2,827-frame snapshots both
reported zero dropped frames. This is a navigation smoke test, not a sustained
playback benchmark. Rust's full Android library suite passed 31 tests; Java
compilation and the JVM audio contract test passed.
