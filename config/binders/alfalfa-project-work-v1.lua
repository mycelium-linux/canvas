local p = event.payload
if type(p) ~= "table" or type(p.beads) ~= "table" then error("alfalfa-project-work-v1 requires beads") end
local live = p.activity_state == "observed"
local value = live and (tonumber(p.beads.open) or 0) or (tonumber(p.beads.linked) or 0)
return {
  kind = "snapshot",
  widget = { kind = "stat", label = live and "Open marbles" or "Linked marbles", value = value, state = (live and (tonumber(p.beads.blocked) or 0) > 0) and "critical" or "neutral" },
  data = { state = live and "ready" or "partial", source = "alfalfa:" .. tostring(p.workspace_id or "?"), detail = live and "live activity" or "catalog references", provenance = "live" },
}
