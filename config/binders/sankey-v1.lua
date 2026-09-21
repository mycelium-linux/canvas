-- Transport-agnostic weighted flow snapshot. Lua shapes producer vocabulary;
-- Rust infers layout and owns all pixels, motion, hover, and theme resolution.
local p = event.payload
if type(p) ~= "table" or type(p.nodes) ~= "table" or type(p.links) ~= "table" then
  error("sankey-v1 requires nodes and links")
end
return {
  kind = "snapshot",
  widget = {
    kind = "sankey",
    title = tostring(p.title or "Flow"),
    nodes = p.nodes,
    links = p.links,
    unit = p.unit and tostring(p.unit) or nil,
  },
  data = {
    state = p.state or "ready",
    source = tostring(p.source or event.source or "flow"),
    detail = p.detail and tostring(p.detail) or nil,
    provenance = "live",
  },
}
