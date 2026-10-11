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

Video surfaces keep a compact 88×44 logical-pixel `Mute` / `Unmute` action.
One press changes only that decoder's gain; it never touches system volume or
other tiles. Repeated stream titles are removed from the visible label, but
the accessibility description retains the exact source and current mute state.
`Unmute` means this stream is muted, not that playback or audible signal was
verified. The ordinary main stream and additional independent streams share
the same label contract. JVM-only `AudioChromeTest` covers that contract.

## Compact shell hierarchy

GFX owns the shared 44-logical-pixel shell row height. Desktop re-exports that
constant; Android reads the same compact row metrics through JNI. Android's
default row is `fungOS / selected workspace / current page / clock`, with a
single workspace action revealing the retained grouped navigation and page
choices. Expanded choices stay in the same horizontal row; they never become
a second stacked toolbar or replace workspace content. Selection collapses the
menu, while MENU focuses the first actual control rather than the brand label.
The TV row shrinks from 56 to 44dp; at density 4 on a 2160px panel, its height
falls from 224px (10.37%) to 176px (8.15%). Desktop's existing 44px row is
unchanged. D-pad control targets remain 44 logical pixels tall. Page controls
and the Library launcher remain available in the expanded navigation.
