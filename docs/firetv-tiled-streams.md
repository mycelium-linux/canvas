# FireTV tiled streams and rotating pages

The Android presentation shim now admits up to four visible `stream` surfaces.
The first retains the existing tuner and shared audio-control binding. Additional
tiles own separate platform `VideoView` decoders, default to muted, and expose
local mute buttons. They are not additional tuner control endpoints. Their mute
state is local and is not yet shared through Canvas control.

Players are retained by surface identity and URL across layout/page changes.
Removed surfaces, workspace changes, and app pause/destroy release additional
decoders. Decoder failures are visible in the affected tile. Hardware limits may
be lower than four simultaneous streams; this is an admission ceiling, not a
hardware performance guarantee.

## Rotation

`config/deploy/firetv/menu.json` declares optional `page_rotation_ms`, keyed by
project ID. The Taig workspace rotates every 15000 ms. Missing/zero intervals
disable rotation; accepted intervals are 5000–300000 ms. It advances the shared
Canvas page state through the existing native page-selection API rather than
keeping a separate Android-only page index. The workspace's main layout is page
zero; its named pages follow and wrap back to zero. Manual page selection delays
the next advance by the configured interval. Rotation runs only while the app is resumed,
the workspace is selected, and the library overlay is closed.

`config/deploy/firetv/taig-verk-pages.json` is a merge fragment, not a replacement
for the retained workspace. Merge its surfaces by identity and append its named
pages while preserving the existing overview and all unrelated workspaces. Use
a fresh endpoint revision and the existing `canvas-tvctl` install contract.
The owner's current combined mill/lathe stream placement remains identical on
every detail page, so its URL and decoder do not restart while the VERK panel
changes. Rebase these video placements on the main layout if the owner changes
it; do not revive an older individual-stream arrangement on just one page.

Only changing static panels crossfade (450 ms). A bounded cache retains up to
twelve PDF/rich-card views keyed by content, geometry, and palette in the selected
rotating workspace. Returning to the overview reuses the rendered PDF bitmap;
it does not reopen/re-rasterize the file. Workspace changes and app pause/destroy
release the cache. Video views never participate in this fade.

The VERK counts come from the retained teacher-inventory PDF identified in the
surface's provenance. Admission boundaries come from `verk/VERK_DATASETS.md`;
architecture details come from `umie/docs/verk-training-architecture.mmd`.
These are documented snapshots, not live losses, activations, training progress,
or proof that exploratory data is admitted. Updating those source documents
requires refreshing these authored detail cards.

## Verification

Compile the Android shim against the installed Android SDK. The timing policy
also has a standalone JVM test with no Android dependencies:

```sh
javac -d /tmp/canvas-page-tests \
  crates/canvas-android/android/dev/fpl/canvas/PageRotation.java \
  crates/canvas-android/android-tests/dev/fpl/canvas/PageRotationTest.java
java -cp /tmp/canvas-page-tests dev.fpl.canvas.PageRotationTest
```

On the TV verify both video tiles advance across the three views, mute controls
stay inside their tiles, and leaving the workspace releases the extra player.
