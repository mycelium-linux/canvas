#!/usr/bin/env bash
# Regression check for cp-server.lua's status matrix:
#   unknown path                        -> 404
#   known path with the wrong method    -> 405
#   valid route                         -> 200
#
# Runs unattended against a throwaway tmux session and an ephemeral port
# (--port 0), so it never touches the operator's live fleet. Startup readiness
# is a blocking read on a FIFO, not a sleep-and-hope loop.
#
# Usage:
#
#   scripts/cp-server-selftest.sh
#
# Exits 0 when all three statuses match, non-zero otherwise.

set -uo pipefail

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
server="$here/cp-server.lua"

session="cp-404-selftest-$$"
tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/cp-404-selftest.$$.XXXXXX")
fifo="$tmpdir/events"
log="$tmpdir/server.log"
pid=""

cleanup() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  exec 3>&- 2>/dev/null || true
  tmux kill-session -t "$session" 2>/dev/null || true
  rm -rf "$tmpdir"
}
trap cleanup EXIT INT TERM

fail() {
  echo "cp-server-selftest: FAIL: $*" >&2
  exit 1
}

command -v luac >/dev/null 2>&1 || fail "luac not found"
command -v curl >/dev/null 2>&1 || fail "curl not found"
command -v tmux >/dev/null 2>&1 || fail "tmux not found"
command -v mkfifo >/dev/null 2>&1 || fail "mkfifo not found"

luac -p "$server" || fail "luac syntax check failed"

tmux new-session -d -s "$session" "sh -c 'echo ready; exec sleep 120'" \
  || fail "could not create scratch tmux session $session"

# Server stderr goes to a FIFO; fd 3 stays open read-write so the writer never
# blocks and the reader never sees EOF. The first "listening" line is the
# readiness signal, so there is no polling sleep.
mkfifo "$fifo"
exec 3<>"$fifo"
"$server" --port 0 --session "$session" --state-file "$tmpdir/state" \
  >"$log" 2>"$fifo" &
pid=$!

port=""
deadline=$((SECONDS + 15))
while ((SECONDS < deadline)); do
  if ! read -r -t 1 -u 3 line; then
    kill -0 "$pid" 2>/dev/null || fail "server exited during startup: $(cat "$log")"
    continue
  fi
  if [[ $line =~ listening\ on\ http://[^:]+:([0-9]+) ]]; then
    port="${BASH_REMATCH[1]}"
    break
  fi
done
[ -n "$port" ] || fail "server did not report a listening port within 15s"

base="http://127.0.0.1:$port"
status() {
  curl -s -o /dev/null -w '%{http_code}' "$@"
}

passed=0
failed=0
check() {
  local name=$1 expected=$2
  shift 2
  local actual
  actual=$(status "$@")
  if [ "$actual" = "$expected" ]; then
    printf 'PASS  %-38s %s\n' "$name" "$actual"
    passed=$((passed + 1))
  else
    printf 'FAIL  %-38s expected %s, got %s\n' "$name" "$expected" "$actual" >&2
    failed=$((failed + 1))
  fi
}

check "GET /api/nope (unknown path)" 404 "$base/api/nope"
check "POST /api/pane/x/capture (wrong method)" 405 -X POST "$base/api/pane/x/capture"
check "GET /api/panes (valid route)" 200 "$base/api/panes"

echo "cp-server-selftest: $passed passed, $failed failed (port $port, session $session)"
[ "$failed" -eq 0 ] || exit 1
