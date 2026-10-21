#!/bin/sh
# Regression check for cp-server.lua's status matrix:
#   unknown path                        -> 404
#   known path with the wrong method    -> 405
#   valid route                         -> 200
#
# Runs against a throwaway tmux session and an ephemeral port, so it never
# touches the operator's live fleet. Usage:
#
#   scripts/cp-server-selftest.sh
#
# Exits 0 when all three statuses match, non-zero (with a diff) otherwise.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/.." && pwd)
server="$here/cp-server.lua"

session="cp-404-selftest-$$"
log=$(mktemp "${TMPDIR:-/tmp}/cp-404-selftest.$$.XXXXXX")
pid=""

cleanup() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  tmux kill-session -t "$session" 2>/dev/null || true
  rm -f "$log"
}
trap cleanup EXIT INT TERM

command -v luac >/dev/null 2>&1 || {
  echo "cp-server-selftest: luac not found" >&2
  exit 2
}
luac -p "$server"

tmux new-session -d -s "$session" "sh -c 'echo ready; exec sleep 120'"

"$server" --port 0 --session "$session" --state-file "$log.state" >"$log" 2>&1 &
pid=$!

port=""
tries=0
while [ -z "$port" ] && [ "$tries" -lt 100 ]; do
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "cp-server-selftest: server exited during startup:" >&2
    cat "$log" >&2
    exit 1
  fi
  port=$(sed -n 's/.*listening on http:\/\/[^:]*:\([0-9][0-9]*\).*/\1/p' "$log" | tail -n 1)
  if [ -z "$port" ]; then
    sleep 0.05
    tries=$((tries + 1))
  fi
done

if [ -z "$port" ]; then
  echo "cp-server-selftest: server did not report a listening port:" >&2
  cat "$log" >&2
  exit 1
fi

status() {
  curl -s -o /dev/null -w '%{http_code}' "$@"
}

base="http://127.0.0.1:$port"
actual=$(printf '%s\n' \
  "$(status "$base/api/nope")" \
  "$(status -X POST "$base/api/pane/selftest/capture")" \
  "$(status "$base/api/panes")")
expected=$(printf '%s\n' 404 405 200)

if [ "$actual" != "$expected" ]; then
  echo "cp-server-selftest: status matrix mismatch" >&2
  echo "expected (unknown path, wrong method, valid route):" >&2
  printf '%s\n' "$expected" >&2
  echo "actual:" >&2
  printf '%s\n' "$actual" >&2
  exit 1
fi

echo "cp-server-selftest: ok (404 unknown path, 405 wrong method, 200 valid route)"
