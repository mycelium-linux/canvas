-- Shared hierarchy contract for treemap and sunburst presentation.
local p = event.payload
if type(p) ~= "table" or type(p.nodes) ~= "table" then
  error("hierarchy-v1 requires nodes")
end
local style = tostring(p.style or "treemap")
if style ~= "treemap" and style ~= "sunburst" then error("unsupported hierarchy style") end
local nodes = canvas.array()
for _, node in ipairs(p.nodes) do
  if type(node) == "table" then
    nodes[#nodes + 1] = {
      id = tostring(node.id or ""),
      label = tostring(node.label or node.name or node.id or ""),
      parent = node.parent and tostring(node.parent) or nil,
      value = tonumber(node.value) or 0,
    }
  end
end
return {
  kind = "snapshot",
  widget = {
    kind = "hierarchy", title = tostring(p.title or "Hierarchy"), style = style,
    nodes = nodes, unit = p.unit and tostring(p.unit) or nil,
    focus = p.focus and tostring(p.focus) or nil,
  },
  data = { state = #nodes > 0 and (p.state or "ready") or "no_data",
    source = tostring(p.source or event.source or "hierarchy"), provenance = "live" },
}
