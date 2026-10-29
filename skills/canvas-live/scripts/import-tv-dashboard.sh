#!/bin/sh
# Revision-safe workspace_import to a Canvas TV. The TV validates the document.
set -eu

usage() {
  echo "usage: import-tv-dashboard.sh ADB_SERIAL DOCUMENT.json" >&2
  echo "  CANVAS_TVCTL overrides the canvas-tvctl binary (default: canvas-tvctl)" >&2
}
[ "$#" -eq 2 ] || { usage; exit 2; }
target=$1
document=$2
[ -f "$document" ] || { echo "import-tv-dashboard: no document $document" >&2; exit 2; }
ctl=${CANVAS_TVCTL:-canvas-tvctl}
command -v "$ctl" >/dev/null 2>&1 || { echo "import-tv-dashboard: $ctl not found" >&2; exit 2; }

revision=$(printf 'inspect\n' | "$ctl" "$target" | jq -r '.revision // empty')
[ -n "$revision" ] || { echo "import-tv-dashboard: inspect returned no revision" >&2; exit 1; }
jq -n \
  --argjson revision "$revision" \
  --arg request "canvas-dashboard-$(date +%s)" \
  --slurpfile document "$document" \
  '{schema_version:1,request_id:$request,expected_revision:$revision,
    command:{command:"workspace_import",document:$document[0],activate:true}}' \
  | "$ctl" "$target"
