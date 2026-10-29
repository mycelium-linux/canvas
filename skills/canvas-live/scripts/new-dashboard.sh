#!/bin/sh
# Scaffold a valid Canvas workspace document without hand-writing the envelope.
# Templates contain no fabricated metrics: all widgets start in no_data state.
set -eu

usage() {
  cat <<'USAGE'
usage: new-dashboard.sh --id ID --project PROJECT [--title TITLE]
                        [--template kpi|list|series|kanban] [--out FILE]

  kpi     a metric board (replace the placeholder metric with a real source)
  list    a status list (the easiest first surface)
  series  a one-series time chart
  kanban  three empty status-list columns in one canvas

  --out - writes to stdout. The generated document is revision 1 and is not
  imported; send it through the owner's revision-bound workspace_import.
USAGE
}

id=""
project=""
title="Dashboard"
template="kpi"
out="-"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --id) id=$2; shift 2 ;;
    --project) project=$2; shift 2 ;;
    --title) title=$2; shift 2 ;;
    --template) template=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "new-dashboard: unknown argument $1" >&2; usage >&2; exit 2 ;;
  esac
done
[ -n "$id" ] || { echo "new-dashboard: --id is required" >&2; exit 2; }
[ -n "$project" ] || { echo "new-dashboard: --project is required" >&2; exit 2; }
command -v jq >/dev/null 2>&1 || { echo "new-dashboard: jq is required" >&2; exit 2; }

data=$(jq -n '{state:"no_data", source:"scaffold", provenance:"live"}')
case "$template" in
  kpi)
    surfaces=$(jq -n --arg id "$id-main" --arg title "$title" --argjson data "$data" '[{
      id:$id, title:$title,
      content:{kind:"widget", spec:{schema_version:2,id:$id,kind:"metric_board",title:$title,
        style:"bullet", metrics:[{label:"Metric", value:0, max:100}], data:$data}},
      inputs:[]
    }]')
    placements='[{"surface_id":"'$id'-main","bounds":{"x":0.02,"y":0.02,"width":0.96,"height":0.96},"layer":0}]'
    ;;
  list)
    surfaces=$(jq -n --arg id "$id-main" --arg title "$title" --argjson data "$data" '[{
      id:$id, title:$title,
      content:{kind:"widget", spec:{schema_version:2,id:$id,kind:"status_list",title:$title,
        nodes:[], data:$data}},
      inputs:[]
    }]')
    placements='[{"surface_id":"'$id'-main","bounds":{"x":0.02,"y":0.02,"width":0.96,"height":0.96},"layer":0}]'
    ;;
  series)
    surfaces=$(jq -n --arg id "$id-main" --arg title "$title" --argjson data "$data" '[{
      id:$id, title:$title,
      content:{kind:"widget", spec:{schema_version:2,id:$id,kind:"time_series",title:$title,
        unit:"value", series:[{label:"Value", points:[{x:0,y:0}]}], data:$data}},
      inputs:[]
    }]')
    placements='[{"surface_id":"'$id'-main","bounds":{"x":0.02,"y":0.02,"width":0.96,"height":0.96},"layer":0}]'
    ;;
  kanban)
    one=$(jq -n --arg id "$id-backlog" --arg title "Backlog" --argjson data "$data" '{id:$id,title:$title,content:{kind:"widget",spec:{schema_version:2,id:$id,kind:"status_list",title:$title,nodes:[],data:$data}},inputs:[]}')
    two=$(jq -n --arg id "$id-wip" --arg title "WIP" --argjson data "$data" '{id:$id,title:$title,content:{kind:"widget",spec:{schema_version:2,id:$id,kind:"status_list",title:$title,nodes:[],data:$data}},inputs:[]}')
    three=$(jq -n --arg id "$id-review" --arg title "Review" --argjson data "$data" '{id:$id,title:$title,content:{kind:"widget",spec:{schema_version:2,id:$id,kind:"status_list",title:$title,nodes:[],data:$data}},inputs:[]}')
    surfaces=$(jq -n --argjson a "$one" --argjson b "$two" --argjson c "$three" '[$a,$b,$c]')
    placements=$(jq -n --arg id "$id" '[
      {surface_id:($id+"-backlog"),bounds:{x:0.0,y:0.0,width:0.333,height:1.0},layer:0},
      {surface_id:($id+"-wip"),bounds:{x:0.333,y:0.0,width:0.334,height:1.0},layer:0},
      {surface_id:($id+"-review"),bounds:{x:0.667,y:0.0,width:0.333,height:1.0},layer:0}
    ]')
    ;;
  *)
    echo "new-dashboard: unknown template $template" >&2
    exit 2
    ;;
esac

document=$(jq -n --arg id "$id" --arg project "$project" --argjson surfaces "$surfaces" --argjson placements "$placements" '{
  schema_version:1,
  theme:{source:"project"},
  background:{kind:"theme"},
  chrome:{title:"hidden",canvas_padding:12,surface_gap:12},
  pages:[], active_page:0,
  id:$id, project_id:$project, revision:1,
  layout:{kind:"canvas",placements:$placements},
  bindings:[], focused_surface_id:null,
  surfaces:$surfaces
}')
if [ "$out" = "-" ]; then
  printf '%s\n' "$document"
else
  mkdir -p "$(dirname "$out")"
  printf '%s\n' "$document" > "$out"
  echo "wrote $out" >&2
fi
