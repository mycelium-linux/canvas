# First-party source snapshots

These are source snapshots, not independently maintained implementations.
Shared component changes belong in their owning repositories first.

* `coherence-native`: `FuturePresentLabs/coherence`, local verified commit
  `8d4c86c38e326299f4f61ac596525bd6fdc25e46`, `crates/coherence-native`.
* `coherence-tokens`: same Coherence commit, `crates/coherence-tokens`.
* `fpl-gfx`: `ajmwagar/gfx`, local verified commit
  `447227efbc518cbb80b81050784785287e0cadb7`, package manifest, source,
  examples, benches and original licenses.

Source/manifests are preserved. Optional upstream examples are included because
Cargo validates their declared paths even when they are not selected. No
upstream target directories, credentials, caches or development config are
vendored. These local commits have not been pushed; update to published package
versions when their owners publish them. Mail-specific composition and mock
behavior live outside these snapshots.
