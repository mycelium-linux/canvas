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
# The server resolves `pi-tmux` at startup and dies if it is missing, which is
# true on a bare CI runner. The status-matrix checks below only exercise routing
# and the tmux fleet snapshot, not pi-tmux actions, so an executable stub is
# enough. A real helper is only used when PI_TMUX_BIN is explicitly set.
stub_tmux="$tmpdir/pi-tmux-stub"
cat >"$stub_tmux" <<'STUB'
#!/bin/sh
# Selftest stub: never touch the operator's real fleet.
printf '[]\n'
STUB
chmod +x "$stub_tmux"
export PI_TMUX_BIN="${PI_TMUX_BIN:-$stub_tmux}"
[ -x "$PI_TMUX_BIN" ] || fail "PI_TMUX_BIN is not executable: $PI_TMUX_BIN"
case "$PI_TMUX_BIN" in
  "$stub_tmux") ;;
  *)
    # Fail loudly rather than silently testing against a real helper.
    [ "${CP_SELFTEST_ALLOW_REAL_PI_TMUX:-}" = "1" ] || \
      fail "refusing to use a non-stub PI_TMUX_BIN ($PI_TMUX_BIN); set CP_SELFTEST_ALLOW_REAL_PI_TMUX=1 to override"
    ;;
esac
echo "cp-server-selftest: PI_TMUX_BIN=$PI_TMUX_BIN (stub unless explicitly overridden)"

# Poison `pi-tmux` on PATH. If the server ever falls back to discovery instead
# of the stub, the poison marker is written and the selftest fails: no real
# tmux fleet may be touched by this test.
poison_dir="$tmpdir/poison-bin"
poison_marker="$tmpdir/real-pi-tmux-called"
mkdir -p "$poison_dir"
cat >"$poison_dir/pi-tmux" <<POISON
#!/bin/sh
echo called >>"$poison_marker"
exit 99
POISON
chmod +x "$poison_dir/pi-tmux"
export PATH="$poison_dir:$PATH"

mkfifo "$fifo"
exec 3<>"$fifo"
"$server" --port 0 --session "$session" --state-file "$tmpdir/state" \
  >"$log" 2>"$fifo" &
pid=$!

# Keep every stderr line so a startup failure reports the server's own message
# instead of an empty stdout log.
stderr_log="$tmpdir/server.err"
: >"$stderr_log"

port=""
deadline=$((SECONDS + 15))
while ((SECONDS < deadline)); do
  if ! read -r -t 1 -u 3 line; then
    if ! kill -0 "$pid" 2>/dev/null; then
      fail "server exited during startup (stderr: $(cat "$stderr_log"); stdout: $(cat "$log"))"
    fi
    continue
  fi
  printf '%s\n' "$line" >>"$stderr_log"
  if [[ $line =~ listening\ on\ http://[^:]+:([0-9]+) ]]; then
    port="${BASH_REMATCH[1]}"
    break
  fi
done
[ -n "$port" ] || fail "server did not report a listening port within 15s (stderr: $(cat "$stderr_log"))"

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

# The stub is the only helper the server was given; if discovery ran anyway the
# poison wrapper would have recorded it.
if [ -e "$poison_marker" ]; then
  fail "real pi-tmux was invoked during the selftest (poison marker present)"
fi

# The scratch session is the only tmux object this script is allowed to touch.
tmux has-session -t "$session" 2>/dev/null || fail "scratch tmux session disappeared"

echo "cp-server-selftest: real pi-tmux not invoked; only scratch session $session touched"
echo "cp-server-selftest: $passed passed, $failed failed (port $port, session $session)"
[ "$failed" -eq 0 ] || exit 1
