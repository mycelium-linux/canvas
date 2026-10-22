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
# Optional: set CI_LOCAL_LOG=/path/to/file to also tee the transcript there.
# Optional: set CI_LOCAL_KEEP_GOING=1 to run every step even after a failure
# (used to collect a full diagnostic transcript; the exit code is still
# non-zero if anything failed).
#
# Exits 2 when a required sibling checkout is missing, non-zero on the first
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

# Resolve a sibling checkout by name. From the repo root the sibling is
# ../<name>; in a .worktrees/<slug> checkout it is either ../<name> (sibling
# worktree of the same repo) or <repo_root>/.worktrees/<name>.
resolve_sibling() {
  local name=$1 candidate
  for candidate in "$repo_root/../$name" "$repo_root/.worktrees/$name"; do
    if [ -d "$candidate" ]; then
      CDPATH= cd -- "$candidate" && pwd
      return 0
    fi
  done
  return 1
}

missing() {
  local name=$1 probe=$2
  cat >&2 <<EOF
ci-local: FATAL: required sibling checkout '$name' not found.

  expected one of:
    $repo_root/../$name
    $repo_root/.worktrees/$name
  and a file at:
    <sibling>/$probe

  The canvas workspace depends on '$name' through path dependencies
  (see Cargo.toml), so the build gate CANNOT run without it. There is no
  skip-and-pass fallback: a green result without these siblings would be a
  lie.

  Fix: clone '$name' next to this checkout (or into .worktrees/), with the
  revision the branch expects, then re-run scripts/ci-local.sh.
EOF
  exit 2
}

gfx=""
unibus=""
gfx=$(resolve_sibling gfx) || missing gfx Cargo.toml
unibus=$(resolve_sibling unibus) || missing unibus crates/stream-descriptors/Cargo.toml

[ -f "$gfx/Cargo.toml" ] || missing gfx Cargo.toml
[ -f "$unibus/crates/stream-descriptors/Cargo.toml" ] || missing unibus crates/stream-descriptors/Cargo.toml

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
