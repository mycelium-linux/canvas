# Legacy branch disposition — 2026-10-26

Scope: every remaining legacy `dock` ref named in the disposition brief. One decision
per ref, with the recomputed unique-commit count against **both** candidate base lines.
No code was changed in any branch being landed; no force-push was used; no push was made
to `master` or `rename/dock-to-canvas`.

## The real base line

Two refs matter, and they are not the same line:

| Ref | SHA | Commits not in the other line |
| --- | --- | --- |
| `origin/master` (GitHub default, `origin/HEAD`) | `cf32a13` | `rename/dock-to-canvas..origin/master` = **219** |
| `rename/dock-to-canvas` (local default) | `f9aa610` | `origin/master..rename/dock-to-canvas` = **44** |

Their merge-base is `f226b87` (`feat(canvas): hot-reload Lua widget compositions`).
Neither contains the other. `origin/master` additionally contains the merged rename
consolidation (`861d9cc`, PR #4) and later CI/control-plane work.

Consequence for the counts below: a ref whose merge-base is on the rename lineage reports
one count against `rename/dock-to-canvas` and a much larger count against `origin/master`
because the latter is missing the rename commits the ref is built on. The actual unique
content is the count against the ref's own lineage. The master-vs-rename default-branch
question is deliberately **not** resolved here.

```text
$ git rev-list --count rename/dock-to-canvas..origin/master
219
$ git rev-list --count origin/master..rename/dock-to-canvas
44
$ git merge-base rename/dock-to-canvas origin/master
f226b8799153fa066f20c1e02a4e33044ede8725
```

## Recomputed counts

Command shape, run for each ref and each base:

```sh
git rev-list --count <base>..<ref>
git merge-base <base> <ref>
```

```text
== codex/gfx-ui-primitives ==
rev-list --count rename/dock-to-canvas..codex/gfx-ui-primitives = 1
merge-base rename/dock-to-canvas codex/gfx-ui-primitives = 8235d48d920852093fece8667a96b092322b90b5
rev-list --count origin/master..codex/gfx-ui-primitives = 35
merge-base origin/master codex/gfx-ui-primitives = f226b8799153fa066f20c1e02a4e33044ede8725
== feat/shared-workspace-native-inbox ==
rev-list --count rename/dock-to-canvas..feat/shared-workspace-native-inbox = 1
merge-base rename/dock-to-canvas feat/shared-workspace-native-inbox = e12ea0de9231c0fa2cfbf39d47f3a5e0d9646f5a
rev-list --count origin/master..feat/shared-workspace-native-inbox = 37
merge-base origin/master feat/shared-workspace-native-inbox = f226b8799153fa066f20c1e02a4e33044ede8725
== recovery/pi-compositor-core ==
rev-list --count rename/dock-to-canvas..recovery/pi-compositor-core = 12
merge-base rename/dock-to-canvas recovery/pi-compositor-core = 65f22c7e8e8225a1bd6318774220a0a9bf29e0c1
rev-list --count origin/master..recovery/pi-compositor-core = 12
merge-base origin/master recovery/pi-compositor-core = 65f22c7e8e8225a1bd6318774220a0a9bf29e0c1
== recovery/pi-gpu-compositor ==
rev-list --count rename/dock-to-canvas..recovery/pi-gpu-compositor = 1
merge-base rename/dock-to-canvas recovery/pi-gpu-compositor = 7e4c503f14fc2b12ae1daf5ec17d5935cd6773b6
rev-list --count origin/master..recovery/pi-gpu-compositor = 1
merge-base origin/master recovery/pi-gpu-compositor = 7e4c503f14fc2b12ae1daf5ec17d5935cd6773b6
== origin/integration/pi-gpu-compositor ==
rev-list --count rename/dock-to-canvas..origin/integration/pi-gpu-compositor = 2
merge-base rename/dock-to-canvas origin/integration/pi-gpu-compositor = ad413654827d1c89fa7a25759a433051aa9d33c7
rev-list --count origin/master..origin/integration/pi-gpu-compositor = 2
merge-base origin/master origin/integration/pi-gpu-compositor = ad413654827d1c89fa7a25759a433051aa9d33c7
== origin/__dolt_remote_info__ ==
rev-list --count rename/dock-to-canvas..origin/__dolt_remote_info__ = 1
merge-base rename/dock-to-canvas origin/__dolt_remote_info__ = (none; unrelated history)
rev-list --count origin/master..origin/__dolt_remote_info__ = 1
merge-base origin/master origin/__dolt_remote_info__ = (none; unrelated history)
```

The brief's lead counts are reproduced exactly, except that the brief attributed the
35/37 figures loosely: they are the counts **against `origin/master`**, not the number
of unique commits on the ref's own line. Against the rename line both refs have exactly
**1** unique commit.

---

## 1. `refs/heads/codex/gfx-ui-primitives` — ABANDON

- Unique commits recomputed: **1** vs `rename/dock-to-canvas` (merge-base `8235d48`);
  **35** vs `origin/master` (merge-base `f226b87`).
- Unique commit: `0e2b7f0 refactor: share WGPU UI primitives through gfx` (2026-10-03).
  It pins `fpl-gfx` at `2a5f378` and moves `canvas-map-wgpu` / `canvas-model-viewer`
  primitives onto it.
- Verdict: **abandon**.
- Superseded by: the public `gfx` repository's shared primitives, adopted on the master
  line in `296a8ee` (`fpl-gfx` at `a37cc03`) and bumped in `93a8446` to
  `ba980e8` — newer than the `2a5f378` pin in this ref. The refactor target also no
  longer exists on `origin/master`: `crates/canvas-map-wgpu` is absent there and the
  model viewer uses `canvas_core` camera constants; the remaining GPU ownership moved
  to `canvas-model-host` / `canvas-widget-host`. `git cherry origin/master
  codex/gfx-ui-primitives` reports the commit as patch-unique, and no patch-equivalent
  commit exists anywhere in `--all`, so the code was not cherry-picked — it was replaced
  by a different, later integration.
- Deletion: **local** — `git branch -D codex/gfx-ui-primitives` (executed).

## 2. `refs/heads/feat/shared-workspace-native-inbox` — LAND

- Unique commits recomputed: **1** vs `rename/dock-to-canvas` (merge-base `e12ea0d`);
  **37** vs `origin/master` (merge-base `f226b87`). The 37 is rename-line ancestry, not
  37 independent changes.
- Unique commit: `f090466 Add read-only Marbles producer for native Canvas workspaces`
  (2026-10-19). Adds the self-contained `crates/canvas-marbles` producer and
  `docs/work-inbox.md`.
- Verdict: **land**.
- What it delivers: a read-only Marbles CLI -> Canvas shared-workspace `task_deck`
  producer. Marbles remains the source of truth; the renderer owns layout/theme/input.
  It ships bounded reads, 16-record capacity, explicit stale/error frames, duplicate and
  cross-tracker rejection, and the documented Fire TV / Studio installation flow.
- Why it still matters: it is the only ref in this set with no superseding implementation
  on either base line. No other ref contains `crates/canvas-marbles` or
  `docs/work-inbox.md`. The committed docs record it as deployed to the `firetv-omni`
  receiver and Studio launchd producer.
- PR: <https://github.com/FuturePresentLabs/dock/pull/8> against `rename/dock-to-canvas`
  (the line its unique commit is measured against). No code changes were made; the
  branch was pushed exactly as committed.
- Deletion: **none now.** The local ref remains because it is checked out in the SPRAWL
  sibling `/Users/ajmwagar/src/fpl/dock-shared-workspace`, which has an uncommitted
  `crates/canvas-marbles/src/lib.rs`; that checkout was read only and was not modified.
  The local ref can be deleted once that sibling is relocated, and the remote branch can
  be deleted after PR #8 merges. This is the one intentional exception to local pruning.

## 3. `refs/heads/recovery/pi-compositor-core` — ABANDON

- Unique commits recomputed: **12** vs `rename/dock-to-canvas`; **12** vs
  `origin/master`. Merge-base is `65f22c7` and is already in both lines, so the 12 are
  genuinely unique on the master line.
- Unique commits (oldest first): `5e7a420`, `0ade341`, `ebf751a`, `87b3631`, `8695a7a`,
  `100b6f6`, `0f7ad55`, `12847a8`, `1572c24`, `f358049`, `5611bf5`, `9aa0b0d`
  (2026-09-13..15): compositor contract, Smithay runner, lifecycle stream, launcher /
  control socket, KMS/DRM output, page-flip loop, fullscreen.
- Verdict: **abandon**.
- Superseded by: master `28bcba5 Recover native Linux compositor into Canvas and retire
  Sway adapter` (2026-10-06), which introduced the current
  `crates/canvas-linux/src/{compositor,launcher,runner}.rs`, `WindowLifecycle`,
  `desktop-compositor-control`, DRM/dmabuf, and the launcher/control-socket stack; plus
  its descendants `5a2145b` (native fullscreen/maximized state, replacing the last ref
  commit `9aa0b0d`), `98d4d09` (libseat DRM/input brokering), `4faeed6` (focus/MRU),
  `5ebd641` (Lua switcher overlay) and `d556da6` (bounded control transport).
- Deletion: **local** — `git branch -D recovery/pi-compositor-core` (executed).

## 4. `refs/heads/recovery/pi-gpu-compositor` — ABANDON

- Unique commits recomputed: **1** vs `rename/dock-to-canvas`; **1** vs `origin/master`.
  Merge-base `7e4c503` is already in both lines.
- Unique commit: `0c43c1b canvas: kiosk widget-host — wpaint tiles + control-socket
  'place'` (2026-09-15).
- Verdict: **abandon**.
- Superseded by: `origin/integration/pi-gpu-compositor` commit `abf92f5 recover
  canvas-linux DRM compositor and widget host`, which carries an expanded version of the
  same widget host (`widgets.rs` 620 vs 567 lines, `wpaint.rs` 792 vs 629 lines), which
  in turn is superseded by master `28bcba5` and the `canvas-widget-host` architecture.
  The compositor-side `place` capability survives on master as
  `CanvasCompositor::place_window` plus the launcher/control path.
- Deletion: **local** — `git branch -D recovery/pi-gpu-compositor` (executed).

## 5. `refs/remotes/origin/integration/pi-gpu-compositor` — ABANDON

- Unique commits recomputed: **2** vs `rename/dock-to-canvas`; **2** vs
  `origin/master`. Merge-base `ad41365` is already in both lines.
- Unique commits: `abf92f5 recover canvas-linux DRM compositor and widget host` and
  `3db0d7e feat(canvas-linux): advertise and import GPU dmabufs` (2026-09-19).
- Verdict: **abandon**.
- Superseded by: master `28bcba5`, verified directly:
  - `git log -S 'DmabufFeedbackBuilder' origin/master -- crates/canvas-linux/src/runner.rs`
    -> `28bcba5` (the dmabuf advertisement/import from `3db0d7e` is present in master).
  - `git log -S 'App launcher / switcher / search' origin/master -- crates/canvas-linux/src/launcher.rs`
    -> `28bcba5` (the recovered launcher/control stack).
  Later master commits (`98d4d09`, `4faeed6`, `5ebd641`, `5a2145b`, ...) extend the same
  implementation.
- Deletion: **remote, pending approval** — see command list at the end. Not executed.

## 6. `refs/remotes/origin/__dolt_remote_info__` — ABANDON

- Unique commits recomputed: **1** total vs either base, but `git merge-base` returns
  nothing because the ref is an unrelated root commit: `30343fb dolt remote info`
  (parentless).
- Content: a single `DOLT_REMOTE.md` with `ref=refs/dolt/data` and the Dolt head/timestamp.
  No product code or docs.
- Verdict: **abandon**. This is a Dolt transport marker, not a branch. The authoritative
  data lives under `refs/dolt/data` and is managed by `bd dolt push/pull`; this marker
  has no consumer and is not a product line.
- Deletion: **remote, pending approval** — see command list at the end. Not executed.

---

## Ref hygiene — before and after

Local legacy refs deleted in this pass: `codex/gfx-ui-primitives`,
`recovery/pi-compositor-core`, `recovery/pi-gpu-compositor`.

```text
$ git branch -D codex/gfx-ui-primitives recovery/pi-compositor-core recovery/pi-gpu-compositor
Deleted branch codex/gfx-ui-primitives (was 0e2b7f0).
Deleted branch recovery/pi-compositor-core (was 9aa0b0d).
Deleted branch recovery/pi-gpu-compositor (was 0c43c1b).
```

### Before

```text
$ git for-each-ref --format='%(refname) %(objectname:short) %(committerdate:short)' refs/heads refs/remotes | sort
refs/heads/ci-gate 7beb706 2026-10-21
refs/heads/codex/gfx-ui-primitives 0e2b7f0 2026-10-03
refs/heads/consolidate-rename 36130d3 2026-10-21
refs/heads/feat/dock-m8x-alfalfa-viewer c4dc35e 2026-10-03
refs/heads/feat/shared-workspace-inbox ebb8a45 2026-09-08
refs/heads/feat/shared-workspace-native-inbox f090466 2026-10-19
refs/heads/master cf32a13 2026-10-22
refs/heads/pi/dock-canvas-chrome ebb8a45 2026-09-08
refs/heads/pi/dock-ci-gate a9a2281 2026-10-21
refs/heads/pi/dock-consolidate a9a2281 2026-10-21
refs/heads/pi/dock-control-plane e1400f0 2026-10-21
refs/heads/pi/dock-legacy-disposition cf32a13 2026-10-22
refs/heads/pi/dock-linux-verify cf32a13 2026-10-22
refs/heads/recovery/pi-compositor-core 9aa0b0d 2026-09-15
refs/heads/recovery/pi-gpu-compositor 0c43c1b 2026-09-15
refs/heads/rename/dock-to-canvas f9aa610 2026-10-19
refs/remotes/origin/HEAD cf32a13 2026-10-22
refs/remotes/origin/__dolt_remote_info__ 30343fb 2026-10-05
refs/remotes/origin/integration/pi-gpu-compositor 3db0d7e 2026-09-19
refs/remotes/origin/master cf32a13 2026-10-22
refs/remotes/origin/pi/canvas-chrome 8969fb1 2026-10-21
refs/remotes/origin/pi/canvas-pre-rename-features f9aa610 2026-10-19
refs/remotes/origin/pi/control-plane a53009b 2026-10-21
refs/remotes/origin/pi/cp-404 3549e58 2026-10-21
refs/remotes/origin/pi/dock-ci-push-trigger 2dba774 2026-10-22
refs/remotes/origin/pi/dock-survey-fmt e6a5ba6 2026-10-21
refs/remotes/origin/rename/dock-to-canvas 25121f0 2026-10-21
```

### After

```text
$ git for-each-ref --format='%(refname) %(objectname:short) %(committerdate:short)' refs/heads refs/remotes | sort
refs/heads/ci-gate 7beb706 2026-10-21
refs/heads/consolidate-rename 36130d3 2026-10-21
refs/heads/feat/dock-m8x-alfalfa-viewer c4dc35e 2026-10-03
refs/heads/feat/shared-workspace-inbox ebb8a45 2026-09-08
refs/heads/feat/shared-workspace-native-inbox f090466 2026-10-19
refs/heads/master cf32a13 2026-10-22
refs/heads/pi/dock-canvas-chrome ebb8a45 2026-09-08
refs/heads/pi/dock-ci-gate a9a2281 2026-10-21
refs/heads/pi/dock-consolidate a9a2281 2026-10-21
refs/heads/pi/dock-control-plane e1400f0 2026-10-21
refs/heads/pi/dock-legacy-disposition cf32a13 2026-10-22
refs/heads/pi/dock-linux-verify cf32a13 2026-10-22
refs/heads/rename/dock-to-canvas f9aa610 2026-10-19
refs/remotes/origin/HEAD cf32a13 2026-10-22
refs/remotes/origin/__dolt_remote_info__ 30343fb 2026-10-05
refs/remotes/origin/integration/pi-gpu-compositor 3db0d7e 2026-09-19
refs/remotes/origin/master cf32a13 2026-10-22
refs/remotes/origin/pi/canvas-chrome 8969fb1 2026-10-21
refs/remotes/origin/pi/canvas-pre-rename-features f9aa610 2026-10-19
refs/remotes/origin/pi/control-plane a53009b 2026-10-21
refs/remotes/origin/pi/cp-404 3549e58 2026-10-21
refs/remotes/origin/pi/dock-ci-push-trigger 2dba774 2026-10-22
refs/remotes/origin/pi/dock-survey-fmt e6a5ba6 2026-10-21
refs/remotes/origin/rename/dock-to-canvas 25121f0 2026-10-21
```

The two remote-tracking refs for remote deletions remain locally until the remote
deletions are approved and a later `git fetch --prune` removes them. `origin/HEAD` and
both base branches are untouched.

## Pending remote deletions — NOT EXECUTED, for approval

These are the exact commands. No `git push --delete` or force-push was run in this pass.

```sh
git push origin --delete __dolt_remote_info__
git push origin --delete integration/pi-gpu-compositor
```

Do not add `--force`; neither deletion rewrites history. They remove only the remote refs
whose dispositions are recorded above.

## Summary

| Ref | Verdict | Replacement / PR | Ref action |
| --- | --- | --- | --- |
| `codex/gfx-ui-primitives` | abandon | `296a8ee`, `93a8446`, public `gfx` `ba980e8` | local deleted |
| `feat/shared-workspace-native-inbox` | land | PR #8 | kept: checked out in SPRAWL sibling |
| `recovery/pi-compositor-core` | abandon | master `28bcba5` + descendants | local deleted |
| `recovery/pi-gpu-compositor` | abandon | `abf92f5`, then master `28bcba5` / `canvas-widget-host` | local deleted |
| `origin/integration/pi-gpu-compositor` | abandon | master `28bcba5` | remote deletion pending |
| `origin/__dolt_remote_info__` | abandon | Dolt `refs/dolt/data` + `bd dolt push/pull` | remote deletion pending |
