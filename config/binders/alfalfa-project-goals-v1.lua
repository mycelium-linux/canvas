local p = event.payload
if type(p) ~= "table" or type(p.goals) ~= "table" then error("alfalfa-project-goals-v1 requires goals") end
local active = tonumber(p.goals.active) or 0
local total = tonumber(p.goals.total) or 0
local ideas = tonumber(p.goals.ideas) or 0
return {
  kind = "snapshot",
  widget = { kind = "stat", label = "Tracked goals", value = total, change = active > 0 and active or nil, state = active > 0 and "good" or "neutral" },
  data = { state = "ready", source = "alfalfa:" .. tostring(p.workspace_id or "?"), detail = tostring(active) .. " active · " .. tostring(ideas) .. " ideas", provenance = "live" },
}
