local p = event.payload
if type(p) ~= "table" or type(p.goals) ~= "table" or type(p.beads) ~= "table"
  or type(p.agents) ~= "table" or type(p.reviewers) ~= "table" then
  error("alfalfa-project-summary-v1 requires goals, beads, agents, and reviewers")
end
local unavailable = p.activity_state ~= "observed"
local state = unavailable and "partial" or "ready"
local function level(value, warning)
  if unavailable then return "neutral" end
  if warning and value > 0 then return "critical" end
  if value > 0 then return "good" end
  return "neutral"
end
return {
  kind = "snapshot",
  widget = {
    kind = "status_list",
    title = tostring(p.name or p.workspace_id or "Project") .. " pulse",
    nodes = {
      { label = "Goals", state = level(p.goals.active or 0, false), detail = tostring(p.goals.active or 0) .. " active · " .. tostring(p.goals.queued or 0) .. " queued · " .. tostring(p.goals.total or 0) .. " total" },
      { label = "Open beads", state = level(p.beads.blocked or 0, true), detail = tostring(p.beads.open or 0) .. " open · " .. tostring(p.beads.in_progress or 0) .. " running · " .. tostring(p.beads.ready or 0) .. " ready" },
      { label = "Agents", state = level(p.agents.running or 0, false), detail = tostring(p.agents.running or 0) .. " running · " .. tostring(p.agents.total or 0) .. " observed" },
      { label = "Reviewers", state = level(p.reviewers.running or 0, false), detail = tostring(p.reviewers.running or 0) .. " running · " .. tostring(p.reviewers.total or 0) .. " observed" },
    },
  },
  data = { state = state, source = "alfalfa:" .. tostring(p.workspace_id or "?"), detail = unavailable and "activity unavailable" or nil, provenance = "live" },
}
