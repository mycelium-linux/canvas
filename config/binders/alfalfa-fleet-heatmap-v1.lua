-- alfalfa-fleet-heatmap-v1: the same roster as an intensity matrix — projects
-- down, stage counts across. Color carries what the status list cannot: how
-- heavy each project's load is relative to the rest of the fleet.
local payload = event.payload
local matrix = payload and payload.matrix
if type(matrix) ~= "table"
  or type(matrix.rows) ~= "table"
  or type(matrix.columns) ~= "table"
  or type(matrix.values) ~= "table"
then
  error("alfalfa-fleet-heatmap-v1 requires payload.matrix with rows, columns, and values")
end

return {
  kind = "snapshot",
  widget = {
    kind = "heatmap",
    title = "Fleet load",
    rows = matrix.rows,
    columns = matrix.columns,
    values = matrix.values,
  },
  data = {
    state = "ready",
    source = "alfalfa:repos",
    detail = "top " .. tostring(#matrix.rows),
    provenance = "live",
  },
}
