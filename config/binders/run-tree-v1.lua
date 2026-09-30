-- Massage any canvas.run.watch.v1 event sequence into the first-party animated
-- execution tree. Transport and bounds stay in Rust; vocabulary stays here.
local p = event.payload
if type(p) ~= "table" or p.schema ~= "canvas.run.watch.v1" or type(p.events) ~= "table" then
  error("run-tree-v1 requires a canvas.run.watch.v1 snapshot")
end

local function glyph(kind, title)
  local name = string.lower(tostring(title or ""))
  if kind == "decision" then return "◇" end
  if kind == "verification" then return "✓" end
  if string.find(name, "extrude", 1, true) then return "↥" end
  if string.find(name, "boolean", 1, true) or string.find(name, "cut", 1, true) then return "−" end
  if kind == "assembly" then return "⬡" end
  return "◆"
end

local nodes, edges = canvas.array(), canvas.array()
local previous = nil
for index, item in ipairs(p.events) do
  local id = tostring(item.id or ("stage-" .. tostring(index)))
  local verdict = tostring(item.verdict or "")
  local state = verdict == "fail" and "critical" or (p.state == "running" and index == #p.events and "warning" or "good")
  nodes[#nodes + 1] = {
    id = id,
    label = tostring(item.title or item.kind or "Stage"),
    detail = tostring(item.kind or "event"),
    glyph = glyph(item.kind, item.title),
    state = state,
    progress = index == #p.events and p.state == "running" and 0.72 or 1.0,
  }
  if previous then edges[#edges + 1] = { from = previous, to = id, active = index == #p.events } end
  previous = id
end

return {
  kind = "snapshot",
  widget = {
    kind = "execution_tree",
    title = tostring((p.run and (p.run.model or p.run.modelId)) or "Run") .. " · live execution",
    nodes = nodes,
    edges = edges,
    active = previous,
  },
  data = {
    state = p.state == "running" and "partial" or "ready",
    source = tostring((p.run and p.run.harness) or "run") .. ":" .. tostring((p.run and p.run.benchmark) or "watch"),
    window = "live",
    provenance = "live",
  },
}
