# Default wallpaper

`undergrowth.png` is the built-in Canvas desktop background for `kind: theme`.
It is a static image: decoded once, retained and cover-scaled. Explicit workspace
images, solid colors, gradients and shaders remain unchanged; unknown backgrounds
still use the theme-gradient fallback. The screensaver remains independent.

The original Rust distance-field kernel is owned by fungOS:
`brand/render-undergrowth.rs` (MIT OR Apache-2.0). Regenerate with:

```sh
rustc -O /path/to/fungos/brand/render-undergrowth.rs -o /tmp/fungos-art
/tmp/fungos-art 1600 1000 /tmp/undergrowth.ppm
ffmpeg -v error -y -i /tmp/undergrowth.ppm -frames:v 1 config/appearance/undergrowth.png
```

Existing installations can import the same wallpaper without replacing binaries:

```sh
printf 'bg-image /absolute/path/undergrowth.png\nws-save PROJECT\n' | canvasctl
```

Use the current project's exact name, and save before changing its background if
you need a rollback copy. This changes presentation only, never surfaces or grants.
