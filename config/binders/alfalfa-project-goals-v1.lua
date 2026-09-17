local p = event.payload
if type(p) ~= "table" or type(p.goals) ~= "table" or type(p.goals.items) ~= "table" then
  error("alfalfa-project-goals-v1 requires goals.items")
end
local entries = {}
for i = 1, #p.goals.items do
  local goal = p.goals.items[i]
  local level = goal.status == "active" and "good" or "neutral"
  entries[#entries + 1] = { timestamp = tostring(goal.id or "?"), message = tostring(goal.outcome or goal.status or "goal"), level = level }
end
if #entries == 0 then entries[1] = { timestamp = "—", message = "no active or queued goals", level = "neutral" } end
return { kind = "snapshot", widget = { kind = "event_log", title = "Goals", entries = entries }, data = { state = "ready", source = "alfalfa:" .. tostring(p.workspace_id or "?"), provenance = "live" } }
