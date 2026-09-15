-- alfalfa-pulse-v1: one project's recent marbles and pull requests as an
-- event log — colored by severity instead of another wall of text columns.
-- The producer already decided the level (blocked screams, in-flight glows,
-- open waits, closed rests); the row order is exactly what the roster sent,
-- never an invented recency. Absence renders as one honest entry.
local payload = event.payload
if type(payload) ~= "table" then
  error("alfalfa-pulse-v1 requires a payload table")
end

local entries = {}
local marbles = payload.marbles or {}
local pulls = payload.pulls or {}
for index = 1, #marbles do
  local marble = marbles[index]
  entries[#entries + 1] = {
    timestamp = tostring(marble[2]),
    message = tostring(marble[4]),
    level = tostring(marble[5] or "neutral"),
  }
end
for index = 1, #pulls do
  local pull = pulls[index]
  entries[#entries + 1] = {
    timestamp = tostring(pull[2]),
    message = "PR " .. tostring(pull[4]) .. " " .. tostring(pull[3]),
    level = tostring(pull[5] or "good"),
  }
end

if #entries == 0 then
  entries[1] = { timestamp = "—", message = "no recent marbles or PRs", level = "neutral" }
end

return {
  kind = "snapshot",
  widget = {
    kind = "event_log",
    title = tostring(payload.project or "?") .. " — recent marbles",
    entries = entries,
  },
  data = {
    state = "ready",
    source = "alfalfa:" .. tostring(payload.project or "?"),
    detail = "backlog " .. tostring(payload.backlog or 0),
    provenance = "live",
  },
}
