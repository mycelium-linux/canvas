local p = event.payload
if type(p) ~= "table" or type(p.series) ~= "table" then
  error("fab-trends-v1 requires series")
end

local series = canvas.array()
for i = 1, #p.series do
  local input = p.series[i]
  local points = canvas.array()
  for j = 1, #(input.points or {}) do
    local point = input.points[j]
    points[#points + 1] = { x = tonumber(point[1]) or 0, y = tonumber(point[2]) or 0 }
  end
  series[#series + 1] = { label = tostring(input.label or "Series"), points = points }
end

return {
  kind = "snapshot",
  widget = {
    kind = "time_series",
    title = "Pipeline activity",
    series = series,
    unit = "jobs",
  },
  data = {
    state = "ready",
    source = "fab",
    window = tostring(p.period or "Rolling window"),
    provenance = "live",
  },
}
