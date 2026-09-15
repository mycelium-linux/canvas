-- alfalfa-overview-v1: the fleet table. One row per project plus a totals
-- line, straight from the canvas-alfalfa producer's payload. "idle" is
-- Alfalfa's own judgement (idle_ratio), shown as a percentage.
local payload = event.payload
if type(payload) ~= "table" or type(payload.rows) ~= "table" then
  error("alfalfa-overview-v1 requires a rows array in the payload")
end

local rows = {
  { "all", tostring(payload.projects) .. " projects", tostring(payload.active),
    tostring(payload.ready), tostring(payload.backlog), tostring(payload.idle) .. " idle" },
}
for index = 1, #payload.rows do
  local row = payload.rows[index]
  rows[#rows + 1] = {
    tostring(row[1]),
    tostring(row[2]),
    tostring(row[3]),
    tostring(row[4]),
    tostring(row[5]),
    tostring(row[6]),
  }
end

return {
  kind = "snapshot",
  widget = {
    kind = "table",
    title = "Alfalfa fleet" .. (payload.truncated and " (top " .. tostring(#payload.rows) .. ")" or ""),
    columns = { "project", "state", "act", "rdy", "bky", "idle" },
    rows = rows,
  },
  data = {
    state = "ready",
    source = "alfalfa:repos",
    provenance = "live",
  },
}
