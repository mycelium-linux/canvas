# Native PDF reading controls

PDF assets use the shared Canvas `AssetRef` contract and immutable, digest-named
app-private bytes. The native Android adapter renders one page off the UI thread
at at most 1920 by 2560 pixels. Presentation zoom reuses that raster; it does not
allocate a four-times-larger bitmap. High zoom therefore magnifies raster pixels.

The remote's OK button cycles Sheet, Drawing, 2x and 4x. Drawing fits the source
drawing bounds with breathing room, centered on those bounds rather than on the
paper. Arrows pan at enlarged magnifications. Rewind and Fast-forward select the
previous/next sheet. Back first restores the whole sheet, then returns to the
previous workspace in this app session. MENU always opens workspace choices.
The explicit Fit sheet and Fit drawing buttons provide the same presentations.
The 56dp toolbar uses existing shared GFX chrome and Coherence palette roles.

For ordinary PDFs without reading regions, Drawing is unavailable and OK uses
the existing full-width reading presentation instead. Page selection and zoom
are local renderer state and reset when leaving the workspace. Returning after
an app restart requires MENU because no previous workspace exists in memory.

An optional `files/assets/PDF_SHA256.viewports.json` contains schema version 1,
the exact `pdf_sha256`, and ordered `pages` entries with normalized `[x,y,w,h]`
`bbox` values and `source_sha256`. Rust requires a regular file of at most64KiB,
at most512 nonempty regions, finite in-page rectangles and lowercase digests.
Android additionally requires the PDF page count to match. Bad metadata is a
visible error; it cannot silently substitute a guessed drawing crop.

For Transmog, the source owner exports a transparent drawing layer containing
geometry, dimensions, leaders and view labels. Paper borders and title blocks
are excluded by the owner at rendering time. The consuming adapter measures the
ink in that layer and binds its digest and region to the retained PDF. It does
not infer geometry by stripping a border from a complete page raster.

Failures show the failed PDF sheet, the owner diagnostic, and Retry/Back/MENU.
OK retries only on user input. No background retry loop is introduced. The
verified raster receives focus after decoding, so OK cannot accidentally select
a workspace button instead of controlling the document.

The JVM-only `android/tests/PdfViewportTest.java` exercises asymmetric drawing
centering, fit bounds, zoom, extreme pan clamping and reset without Android.
Compile it together with `android/dev/fpl/canvas/PdfViewport.java`, then run
`dev.fpl.canvas.PdfViewportTest`. Qualify actual Android remote interactions and
screenshots separately; a JVM test does not establish TV playback or rendering.
