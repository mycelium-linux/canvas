-- alfalfa-pulse-v1: one project's life — marbles and pull requests, with the
-- stage counts as header context. Marbles use Alfalfa's own word; the row
-- order is exactly what the roster sent (newest activity first), never a
-- invented recency. A project with neither still renders: absence is shown,
-- not hidden.
local payload = event.payload
if type(payload) ~= "table" then
  error("alfalfa-pulse-v1 requires a payload table")
end

local rows = {}
local marbles = payload.marbles or {}
local pulls = payload.pulls or {}
for index = 1, #marbles do
  local marble = marbles[index]
  rows[#rows + 1] = {
    tostring(marble[1]),
    tostring(marble[2]),
    tostring(marble[3]),
    tostring(marble[4]),
  }
end
for index = 1, #pulls do
  local pull = pulls[index]
  rows[#rows + 1] = {
    tostring(pull[1]),
    tostring(pull[2]),
    tostring(pull[3]),
    tostring(pull[4]),
  }
end

if #rows == 0 then
  rows[1] = { "—", "no recent marbles or PRs", "", "" }
end

return {
  kind = "snapshot",
  widget = {
    kind = "table",
    title = tostring(payload.project or "?") .. " pulse",
    columns = { "kind", "ref", "state", "title" },
    rows = rows,
  },
  data = {
    state = "ready",
    source = "alfalfa:" .. tostring(payload.project or "?"),
    detail = tostring(payload.active or 0) .. " active · "
      .. tostring(payload.ready or 0) .. " ready · "
      .. tostring(payload.backlog or 0) .. " backlog · idle "
      .. tostring(payload.idle or "-"),
    provenance = "live",
  },
}
