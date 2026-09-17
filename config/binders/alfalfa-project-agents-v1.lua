local p = event.payload
if type(p) ~= "table" or type(p.agents) ~= "table" then error("alfalfa-project-agents-v1 requires agents") end
local live = p.activity_state == "observed"
return {
  kind = "snapshot",
  widget = { kind = "stat", label = "Running agents", value = tonumber(p.agents.running) or 0, state = live and "good" or "neutral" },
  data = { state = live and "ready" or "no_data", source = "alfalfa:" .. tostring(p.workspace_id or "?"), detail = live and (tostring(p.agents.total or 0) .. " observed") or "activity unavailable", provenance = "live" },
}
