-- Correlation is a semantic specialization of Canvas's shared matrix heatmap.
local p = event.payload
if type(p) ~= "table" or type(p.labels) ~= "table" or type(p.values) ~= "table" then
  error("correlation-matrix-v1 requires labels and values")
end

local labels, values = canvas.array(), canvas.array()
for _, label in ipairs(p.labels) do labels[#labels + 1] = tostring(label) end
for _, input_row in ipairs(p.values) do
  local row = canvas.array()
  for _, value in ipairs(input_row) do
    row[#row + 1] = math.max(-1, math.min(1, tonumber(value) or 0))
  end
  values[#values + 1] = row
end

return {
  kind = "snapshot",
  widget = {
    kind = "heatmap",
    title = tostring(p.title or "Correlation matrix"),
    rows = labels,
    columns = labels,
    values = values,
    style = "matrix",
    unit = p.unit and tostring(p.unit) or "ρ",
  },
  data = {
    state = #labels > 0 and (p.state or "ready") or "no_data",
    source = tostring(p.source or event.source or "correlation"),
    provenance = "live",
  },
}
