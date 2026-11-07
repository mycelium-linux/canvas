# Native file actions

Canvas implements a DragShift-inspired radial menu in Rust and iced. DragShift
is the interaction reference, not a runtime dependency:
<https://github.com/llamaboi55/DragShift> (reviewed at `49a95b8`).

Drag files over a Canvas window and press Shift. Move onto a slice and drop to
choose it. Tap Shift again, or Tab, to switch between Convert and Tools. The
center and the area outside the ring do not select an action. Escape or a
right-click cancels. Dropping without a selected slice leaves the menu open;
click a slice or press its number to choose. Up to 32 files share one action.

Convert offers Open in Canvas, PNG, JPEG and WebP. Tools offers Open in Canvas,
half size, clockwise 90° rotation and grayscale. Image actions accept Canvas's
PNG/JPEG/WebP inputs and reuse its bounded decoder. JPEG exports flatten
transparency onto white. Tools write PNG to avoid another lossy encoding.
Exports are saved beside the source as `NAME-canvas-ACTION-N.EXT`, reserving a
new filename without replacing an existing file. Originals remain unchanged.
Successful results open as ordinary Canvas surfaces. Partial batch failures are
reported with the affected file paths. If the project changes while work runs,
results remain on disk and are not inserted into the new workspace.

The control interface opens the same native menu for explicit local files:

```sh
echo 'file-actions "/absolute/path/to/image.png"' | cargo run -p canvas --bin canvasctl
```

The equivalent JSON command is
`{"kind":"file_actions","paths":["/absolute/path/to/image.png"]}` inside the
normal Canvas control request envelope. The receipt reports `file_actions_shown`
and the number of files; it acknowledges presentation, not a completed export.

The trigger is scoped to Canvas windows. It does not monitor global input or
Finder. iced 0.14's Wayland backend does not emit file-hover/drop events; use
the control entry point there. macOS and X11 use their native window events.
PDF/document conversion, crop/annotation, target-size compression, system-wide
triggering and Undo are not implemented in this slice.

## Verification

Automated tests cover radial hit targets and dead zones, batch collection,
explicit selection after drop, cancellation, batch limits, valid image output,
transparency, dimensions, source preservation and output collisions.

For a manual check, drag a PNG into Canvas, tap Shift, move onto JPEG, and drop.
Confirm that a separately named JPEG opens and the PNG is unchanged. Switch to
Tools and repeat with Half size. Drop in the center and confirm no file is
written until a slice is chosen. Escape should leave the workspace unchanged.
Use the control command above to exercise the same menu without a drag.
