-- Normalize transport/service telemetry into Canvas's shared live graph.
-- The producer owns facts; Rust owns layout, animation, and theme semantics.
local p = event.payload
if type(p) ~= "table" or type(p.nodes) ~= "table" or type(p.edges) ~= "table" then
  error("service-topology-v1 requires nodes and edges")
end

local nodes, edges = canvas.array(), canvas.array()
for i, node in ipairs(p.nodes) do
  if type(node) == "table" then
    local health = tostring(node.health or node.state or "neutral")
    nodes[#nodes + 1] = {
      id = tostring(node.id or ("node-" .. tostring(i))),
      label = tostring(node.label or node.name or node.id or ("Node " .. tostring(i))),
      group = node.group and tostring(node.group) or (node.region and tostring(node.region) or nil),
      heat = math.max(0, math.min(1, tonumber(node.activity or node.heat) or 0)),
      state = health == "healthy" and "good"
        or (health == "degraded" and "warning" or (health == "failed" and "critical" or health)),
    }
  end
end
for _, edge in ipairs(p.edges) do
  if type(edge) == "table" then
    edges[#edges + 1] = {
      from = tostring(edge.from or edge.source or ""),
      to = tostring(edge.to or edge.target or ""),
      heat = math.max(0, math.min(1, tonumber(edge.activity or edge.heat) or 0)),
    }
  end
end

return {
  kind = "snapshot",
  widget = { kind = "neural_graph", title = tostring(p.title or "Service topology"), nodes = nodes, edges = edges },
  data = {
    state = #nodes > 0 and (p.state or "ready") or "no_data",
    source = tostring(p.source or event.source or "topology"),
    detail = p.detail and tostring(p.detail) or nil,
    provenance = "live",
  },
}
