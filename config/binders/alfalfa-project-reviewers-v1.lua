local p = event.payload
if type(p) ~= "table" or type(p.reviewers) ~= "table" then error("alfalfa-project-reviewers-v1 requires reviewers") end
local live = p.activity_state == "observed"
return {
  kind = "snapshot",
  widget = { kind = "stat", label = "Running reviewers", value = tonumber(p.reviewers.running) or 0, state = live and "good" or "neutral" },
  data = { state = live and "ready" or "no_data", source = "alfalfa:" .. tostring(p.workspace_id or "?"), detail = live and (tostring(p.reviewers.total or 0) .. " observed") or "activity unavailable", provenance = "live" },
}
