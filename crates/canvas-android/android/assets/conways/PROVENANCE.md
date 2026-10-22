# Original Conway app

Frozen from https://github.com/ajmwagar/conways/tree/c7a4aadb8d658eed2f3fcaa1c0b4801a5dcaf80f
(the original GitHub Pages build). HTML, JavaScript bundles and WASM are unmodified.
Original MIT/Apache-2.0 license texts are included. ConwaysView supplies only
offline hosting, bounded remote controls and Canvas-themed presentation.

No JavaScript native bridge, credentials, network fetches or file access are
exposed. Unknown resource requests are rejected. Simulation is the original
Rust/WASM universe, not another implementation in Canvas.

## Packaging and use

Compile `ConwaysView.java` alongside the Android shim. Copy this entire folder
to `assets/conways/` in the APK before zipalign/signing, and include the shared
`config/deploy/firetv/menu.json` as `assets/canvas-menu.json`. Install the
`config/deploy/firetv/conways.json` document with project identity `conways`.
Other platforms retain this surface and show their explicit unsupported
placeholder until they have an appropriate host.

The FireTV remote uses Play/Pause, Fast-forward (randomize), and Rewind (clear).
Mouse clicks retain the original cell editing; Shift-click inserts a glider.
Switching away destroys the local WebView; backgrounding pauses its timers.

The shell samples a 64x64 cell-center raster four times per second. Exact
states (not hashes) are retained in a bounded 32-observation ring. Sixteen
consecutive observations revisiting that ring trigger a visible 1.5-second
settlement hold and reseed. Empty fields, still lifes and short repeating
cycles are detected; cycles beyond this window are not guaranteed. Unchanged
generation counters and manual clears/pauses never trigger reseeding.
`android/tests/LifeSettlementTest.java` is a plain JVM regression test.
