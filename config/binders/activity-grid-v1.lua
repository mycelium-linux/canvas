-- Generic calendar/activity binder. Producers own observations and labels;
-- Canvas owns theme, quantization, layout, caching, and drawing.
local p = event.payload
if type(p) ~= "table"
  or type(p.rows) ~= "table"
  or type(p.columns) ~= "table"
  or type(p.values) ~= "table"
then
  error("activity-grid-v1 requires rows, columns, and values")
end

return {
  kind = "snapshot",
  widget = {
    kind = "heatmap",
    style = "activity",
    title = tostring(p.title or "Activity"),
    rows = p.rows,
    columns = p.columns,
    values = p.values,
    unit = p.unit and tostring(p.unit) or nil,
  },
  data = {
    state = p.state or "ready",
    source = tostring(p.source or event.source or "activity"),
    detail = p.detail and tostring(p.detail) or nil,
    provenance = "live",
  },
}
