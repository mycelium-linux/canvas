-- alfalfa-overview-v1: the fleet as a status list — one dot per project,
-- colored by what it is doing (working glows, paused warns, resting is
-- neutral), counts and Alfalfa's idle judgement in the detail line. A totals
-- row on top; every value is a fold of the same roster the pulses use.
local payload = event.payload
if type(payload) ~= "table" or type(payload.rows) ~= "table" then
  error("alfalfa-overview-v1 requires a rows array in the payload")
end

local function level(state, act)
  if state == "paused" then return "warning" end
  if type(act) == "number" and act > 0 then return "good" end
  return "neutral"
end

local nodes = {
  {
    label = "fleet (" .. tostring(payload.projects or 0) .. ")",
    state = "neutral",
    detail = "active " .. tostring(payload.active or 0)
      .. " · ready " .. tostring(payload.ready or 0)
      .. " · backlog " .. tostring(payload.backlog or 0)
      .. " · idle " .. tostring(payload.idle or 0),
  },
}
for index = 1, #payload.rows do
  local row = payload.rows[index]
  nodes[#nodes + 1] = {
    label = tostring(row[1]),
    state = level(tostring(row[2]), row[3]),
    detail = tostring(row[2]) .. " · a" .. tostring(row[3])
      .. " r" .. tostring(row[4]) .. " b" .. tostring(row[5])
      .. " · " .. tostring(row[6]) .. " idle",
  }
end

return {
  kind = "snapshot",
  widget = {
    kind = "status_list",
    title = "Alfalfa fleet",
    nodes = nodes,
  },
  data = {
    state = "ready",
    source = "alfalfa:repos",
    provenance = "live",
  },
}
