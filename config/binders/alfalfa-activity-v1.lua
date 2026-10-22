-- alfalfa-activity-v1: the same pulse stream, rendered as a gauge instead:
-- how alive a project is right now (100 - idle%). Two bindings, one stream,
-- two shapes — the point of binder-split producers. No idle ratio reported
-- is no activity claimed: the gauge reads zero and says so.
local payload = event.payload
if type(payload) ~= "table" then
  error("alfalfa-activity-v1 requires a payload table")
end

local activity = payload.activity
if type(activity) ~= "number" then
  activity = 0
end

return {
  kind = "snapshot",
  widget = {
    kind = "gauge",
    value = activity,
    label = tostring(payload.project or "?") .. " activity",
  },
  data = {
    state = "ready",
    source = "alfalfa:" .. tostring(payload.project or "?"),
    detail = tostring(payload.active or 0) .. " active / "
      .. tostring(payload.ready or 0) .. " ready",
    provenance = "live",
  },
}
