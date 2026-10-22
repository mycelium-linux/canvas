#!/usr/bin/env bash
# Local Rust build/test gate for canvas.
#
# The hosted workflow (.github/workflows/ci.yml) can only run checks with no
# private-dependency surface. This script is the other half: the full
# fmt/check/clippy/test/smoke gate against the sibling `gfx` and `unibus`
# checkouts that the workspace needs.
#
# Usage (from the repo root, works inside a .worktrees/<slug> checkout too):
#
#   scripts/ci-local.sh
#
# Siblings are resolved by REPO IDENTITY: a candidate is accepted only when its
# `origin` remote is the expected repository and the revision pinned in
# Cargo.toml is present locally. A copied snapshot or a same-named worktree of
# another repository (for example a dock worktree called `unibus`) is ignored
# and reported, never used. Override with CI_LOCAL_GFX / CI_LOCAL_UNIBUS.
#
# Optional: set CI_LOCAL_LOG=/path/to/file to also tee the transcript there.
# Optional: set CI_LOCAL_KEEP_GOING=1 to run every step even after a failure
# (used to collect a full diagnostic transcript; the exit code is still
# non-zero if anything failed).
#
# Exits 2 when no valid sibling checkout can be found, non-zero on the first
# failed step (or after all steps with CI_LOCAL_KEEP_GOING), and 0 only when
# every step passed.
#
# Layout note: the workspace's Cargo.toml points at `../gfx` and
# `../unibus`. Because `gfx` has no `[workspace]` table, a `gfx` checkout that
# lives *inside* the canvas repo tree (for example a `.worktrees/gfx`
# symlink) is mistaken by Cargo for a member of the canvas workspace and the
# build is rejected. The siblings must therefore sit outside the canvas
# checkout, e.g. `/src/fpl/gfx` next to `/src/fpl/dock`, or in a dedicated run
# directory that mirrors that layout.

set -euo pipefail

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$here/.." && pwd)
cd "$repo_root"

log_path=${CI_LOCAL_LOG:-}
if [ -n "$log_path" ]; then
  # Duplicate the transcript without hiding it from the caller.
  exec > >(tee -a "$log_path") 2>&1
fi

step=""
failed_steps=()
keep_going=${CI_LOCAL_KEEP_GOING:-}
on_exit() {
  local status=$?
  if [ "$status" -ne 0 ] && [ -n "$step" ] && [ -z "$keep_going" ]; then
    echo "ci-local: FAILED at $step (exit $status)" >&2
  fi
}
trap on_exit EXIT

# Resolve a sibling checkout by REPO IDENTITY, never by path presence alone.
# A stale checkout (for example a dock worktree that happens to be named
# `unibus`) must not satisfy the gate: it has the wrong remote and/or lacks the
# pinned revision. Accept a candidate only when its `origin` matches the
# expected repository and the revision pinned in Cargo.toml is present.
#
# Env overrides: CI_LOCAL_GFX / CI_LOCAL_UNIBUS take priority when set.

require_dep() {
  local name=$1 expect_remote=$2 pin=$3 candidate remote
  shift 3
  for candidate in "$@"; do
    [ -d "$candidate" ] || continue
    if ! git -C "$candidate" rev-parse --git-dir >/dev/null 2>&1; then
      echo "ci-local: ignoring $name candidate $candidate: not a git checkout" >&2
      continue
    fi
    remote=$(git -C "$candidate" remote get-url origin 2>/dev/null || true)
    case "$remote" in
      *"$expect_remote"*) ;;
      *)
        echo "ci-local: ignoring $name candidate $candidate: origin '$remote' is not *$expect_remote*" >&2
        continue
        ;;
    esac
    if ! git -C "$candidate" cat-file -e "${pin}^{commit}" 2>/dev/null; then
      echo "ci-local: ignoring $name candidate $candidate: pinned revision $pin is not present (fetch it)" >&2
      continue
    fi
    printf '%s\n' "$candidate"
    return 0
  done
  return 1
}

missing_dep() {
  local name=$1 expect_remote=$2 pin=$3 probe=$4
  shift 4
  cat >&2 <<EOF
ci-local: FATAL: no valid '$name' sibling checkout found.

  A valid checkout must:
    - be a git checkout (not a copied snapshot and not a same-named worktree
      of another repository),
    - have an 'origin' remote containing: $expect_remote
    - contain the revision pinned in Cargo.toml: $pin
    - contain: $probe

  candidates tried:
$(printf '    %s\n' "$@")

  The canvas workspace depends on '$name' through path dependencies (see
  Cargo.toml), so the build gate CANNOT run without it. There is no
  skip-and-pass fallback: a green result without these siblings would be a
  lie.

  Fix: clone the real repository next to this checkout (or point
  CI_LOCAL_${name^^} at it), fetch the pinned revision, then re-run
  scripts/ci-local.sh.
EOF
  exit 2
}

# Pinned revisions come from Cargo.toml (single source of truth), not from a
# hardcoded copy that can drift.
gfx_pin=$(sed -n 's/.*fpl-gfx = { git = "https:\/\/github.com\/ajmwagar\/gfx.git", rev = "\([0-9a-f]*\)".*/\1/p' Cargo.toml | head -n1)
unibus_pin=$(sed -n 's/.*unibus = { git = "ssh:\/\/git@github.com\/FuturePresentLabs\/unibus.git", rev = "\([0-9a-f]*\)".*/\1/p' Cargo.toml | head -n1)
if [ -z "$gfx_pin" ] || [ -z "$unibus_pin" ]; then
  echo "ci-local: FATAL: could not read pinned gfx/unibus revisions from $repo_root/Cargo.toml" >&2
  exit 2
fi

gfx_candidates=()
unibus_candidates=()
[ -n "${CI_LOCAL_GFX:-}" ] && gfx_candidates+=("$CI_LOCAL_GFX")
[ -n "${CI_LOCAL_UNIBUS:-}" ] && unibus_candidates+=("$CI_LOCAL_UNIBUS")
for base in "$repo_root/.." "$repo_root/.worktrees" "$repo_root/../.." "$repo_root/../../.." "$repo_root/../../../.."; do
  gfx_candidates+=("$base/gfx")
  unibus_candidates+=("$base/unibus" "$base/unibus-integration" "$base/unibus-world-network")
done

gfx=$(require_dep gfx "ajmwagar/gfx" "$gfx_pin" "${gfx_candidates[@]}") || \
  missing_dep gfx "ajmwagar/gfx" "$gfx_pin" Cargo.toml "${gfx_candidates[@]}"
unibus=$(require_dep unibus "FuturePresentLabs/unibus" "$unibus_pin" "${unibus_candidates[@]}") || \
  missing_dep unibus "FuturePresentLabs/unibus" "$unibus_pin" crates/stream-descriptors/Cargo.toml "${unibus_candidates[@]}"

# Cargo resolves the workspace's path dependencies itself (`../gfx` and
# `../unibus` from this manifest). A verified checkout somewhere else is NOT
# what Cargo will build, so accepting it would be a false green. Require the
# selected checkout to be exactly the path Cargo uses.
require_cargo_path() {
  local name=$1 selected=$2 expected=$3 selected_real expected_real
  selected_real=$(CDPATH= cd -- "$selected" && pwd -P)
  if [ ! -d "$expected" ]; then
    echo "ci-local: FATAL: Cargo expects $name at $expected, but it does not exist." >&2
    echo "ci-local: place the verified checkout there (clone or symlink), then re-run." >&2
    exit 2
  fi
  expected_real=$(CDPATH= cd -- "$expected" && pwd -P)
  if [ "$selected_real" != "$expected_real" ]; then
    cat >&2 <<EOF
ci-local: FATAL: verified $name ($selected_real) is not the checkout Cargo
  will build ($expected_real, from Cargo.toml's '../$name').

  Refusing to run: a passing gate against a different tree is a false green.
  Make Cargo's path point at the verified checkout (clone or symlink it there),
  or run from a checkout whose parent contains the real siblings.
EOF
    exit 2
  fi
}

require_cargo_path gfx "$gfx" "$repo_root/../gfx"
require_cargo_path unibus "$unibus" "$repo_root/../unibus"

[ -f "$gfx/Cargo.toml" ] || missing_dep gfx "ajmwagar/gfx" "$gfx_pin" Cargo.toml "$gfx"
[ -f "$unibus/crates/stream-descriptors/Cargo.toml" ] || \
  missing_dep unibus "FuturePresentLabs/unibus" "$unibus_pin" crates/stream-descriptors/Cargo.toml "$unibus"

# A real unibus checkout has service_adoption.rs; a stale dock worktree named
# 'unibus' does not. Identity checks above already catch that case, but keep
# the concrete probe so the diagnostic names the missing source file.
[ -f "$unibus/crates/unibus/src/service_adoption.rs" ] || \
  missing_dep unibus "FuturePresentLabs/unibus" "$unibus_pin" crates/unibus/src/service_adoption.rs "$unibus"

git_head() {
  local dir=$1
  if [ -d "$dir/.git" ] || git -C "$dir" rev-parse --git-dir >/dev/null 2>&1; then
    git -C "$dir" rev-parse --short HEAD 2>/dev/null || echo "unknown"
  else
    echo "not-a-git-repo"
  fi
}

echo "ci-local: repo root       = $repo_root"
echo "ci-local: gfx sibling     = $gfx ($(git_head "$gfx"))"
echo "ci-local: unibus sibling  = $unibus ($(git_head "$unibus"))"

run_step() {
  local name=$1 status=0
  shift
  step=$name
  echo
  echo "ci-local: === $name ==="
  echo "ci-local: \$ $*"
  "$@" || status=$?
  step=""
  if [ "$status" -ne 0 ]; then
    echo "ci-local: FAILED at $name (exit $status)" >&2
    failed_steps+=("$name")
    if [ -z "$keep_going" ]; then
      exit "$status"
    fi
  fi
  return 0
}

run_step "fmt" cargo fmt --all -- --check
run_step "check" cargo check --workspace --all-targets
run_step "clippy" cargo clippy --workspace --all-targets -- -D warnings
run_step "test" cargo test --workspace
run_step "smoke" cargo run -p canvas-core --bin canvas-core-ctl -- --help

if [ "${#failed_steps[@]}" -gt 0 ]; then
  echo
  echo "ci-local: FAILED (${#failed_steps[@]} step(s): ${failed_steps[*]})" >&2
  exit 1
fi

echo
echo "ci-local: OK"
