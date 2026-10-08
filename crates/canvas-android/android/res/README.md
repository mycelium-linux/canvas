# fungOS launcher artwork

`drawable-nodpi/fungos_icon.png`: square mark + fungOS wordmark, 512 × 512.
`drawable-xhdpi/fungos_banner.png`: wide mark + fungOS wordmark, 320 × 180.
The manifest selects both so TV and settings launchers can use their native format.

Original procedural source: `fungos/brand/render-undergrowth.rs` (MIT OR Apache-2.0).
The lettering is original vector geometry, with no font dependency. Generate the
app SVGs and raster PNGs using the [fungOS brand runbook](https://github.com/ajmwagar/fungos/tree/main/brand).
Scale its 640 × 360 TV PNG to 320 × 180 for the xhdpi banner. Do not copy the kernel.

Compile this resource directory with `aapt2 compile --dir android/res` before
linking the APK. Assets are build-time outputs; Canvas has no runtime fungOS
dependency. The project name and logo are marks; see fungOS' `TRADEMARKS.md`.

`drawable-nodpi/undergrowth.png` is byte-identical to the desktop's
`config/appearance/undergrowth.png`. The TV presentation layer draws it with
center-crop behind surfaces only for the default `theme` background. Explicit
shader backgrounds remain shader-owned; this adds no shader workload or remote
image fetch. Missing bundled artwork fails visibly during startup.

Keep Java and JNI helpers matched to the native release. A newer chrome helper
must not be packaged with an older library lacking its native entry point.
The 2026-10-08 wallpaper qualification retained the installed native libraries
and legacy chrome, installed in place with the existing signer, checked the
wallpaper on the clock workspace, then restored the selected machining stream.
