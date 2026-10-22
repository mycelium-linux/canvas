-- Turn Transmog build facts into the particular Canvas view bound to this
-- stream. The producer owns facts; this Lua file owns presentation.
local p = event.payload
if type(p) ~= "table" or type(p.steps) ~= "table" then
  error("transmog build status requires steps")
end

local total = p.total or #p.steps
local done = p.done or #p.steps
local state = p.complete and "ready" or "partial"
local data = {
  state = state,
  source = "transmog.build.status.v1",
  window = "live",
}

-- Operation glyphs belong to the view, not the build producer. Prefer the
-- most specific semantic name Transmog supplies so a future Boolean subtype
-- (cut/union/intersect) automatically replaces the generic Boolean diamond.
local function operation_icon(step)
  local name = string.lower(step.operation or step.action or step.subtype or step.kind or "")
  if string.find(name, "extrude", 1, true) then return "↥" end
  if string.find(name, "cut", 1, true) or string.find(name, "subtract", 1, true) then return "−" end
  if string.find(name, "union", 1, true) or string.find(name, "join", 1, true) then return "+" end
  if string.find(name, "intersect", 1, true) then return "∩" end
  if string.find(name, "revolve", 1, true) then return "↻" end
  if string.find(name, "fillet", 1, true) then return "◒" end
  if string.find(name, "chamfer", 1, true) then return "◩" end
  if string.find(name, "shell", 1, true) then return "◫" end
  if string.find(name, "hole", 1, true) or string.find(name, "drill", 1, true) then return "⊖" end
  if string.find(name, "pattern", 1, true) or string.find(name, "array", 1, true) then return "⠿" end
  if string.find(name, "boolean", 1, true) then return "◇" end
  return "•"
end

local function operation_name(step)
  return step.operation or step.action or step.subtype or step.kind or "Operation"
end

-- Model timelines are not widgets, but use the same binder so operation
-- semantics have exactly one owner. Rust only receives normalized controls.
if event.stream == "transmog-timeline" then
  local items = canvas.array()
  for i = #p.steps, 1, -1 do
    local step = p.steps[i]
    local index = step.index or (i - 1)
    items[#items + 1] = {
      steps_back = math.max(0, total - 1 - index),
      glyph = operation_icon(step),
      label = "#" .. tostring(index + 1),
      component = step.component,
      feature = step.feature,
    }
  end
  return { stream_id = p.stream_id or "transmog-step", items = items }
end

if event.stream == "transmog-fan" then
  local nodes = canvas.array()
  local edges = canvas.array()
  local previous = nil
  for i = #p.steps, 1, -1 do
    local step = p.steps[i]
    local id = "s" .. tostring(step.index or i)
    nodes[#nodes + 1] = {
      id = id,
      label = operation_icon(step) .. " #" .. tostring(step.index or i) .. " " .. operation_name(step),
      group = step.csg or "feature",
      heat = 0.85,
      state = "good",
    }
    if previous then
      edges[#edges + 1] = { from = previous, to = id, heat = 0.7 }
    end
    previous = id
  end
  return {
    kind = "snapshot",
    widget = {
      kind = "dag_graph",
      title = done .. "/" .. total .. " operations · " .. p.material .. " / " .. p.process,
      nodes = nodes,
      edges = edges,
    },
    data = data,
  }
end

if event.stream == "transmog-brief" then
  return {
    kind = "snapshot",
    widget = {
      kind = "event_log",
      title = "Build intent · " .. (p.material_source or "unknown"),
      entries = canvas.array({
        { timestamp = "material", message = p.material .. " · " .. p.material_source .. " selection", level = "good" },
        { timestamp = "process", message = p.process .. " · target process", level = "good" },
        { timestamp = "source", message = p.source, level = "neutral" },
      }),
    },
    data = data,
  }
end

if event.stream == "transmog-steps" then
  local entries = canvas.array()
  for _, step in ipairs(p.steps) do
    entries[#entries + 1] = {
      timestamp = "step " .. tostring(step.index or "?"),
      message = operation_icon(step) .. " " .. operation_name(step) .. " · feature " .. tostring(step.feature or 0)
        .. " · " .. tostring(step.tris or 0) .. " tris · " .. tostring(step.ms or 0) .. " ms",
      level = step.slow and "warning" or "good",
    }
  end
  return {
    kind = "snapshot",
    widget = { kind = "event_log", title = "Build steps · " .. done .. "/" .. total, entries = entries },
    data = data,
  }
end

if event.stream == "transmog-confidence" then
  local completed = total == 0 and 0 or done * 100 / total
  return {
    kind = "snapshot",
    widget = {
      kind = "ranked_bar",
      title = p.complete and "Build complete" or "Building",
      bars = canvas.array({
        { label = "operations complete", value = completed, state = "good" },
        { label = p.material .. " authored", value = 100, state = "good" },
        { label = p.process .. " selected", value = 100, state = "good" },
      }),
      unit = "%",
    },
    data = data,
  }
end

error("unsupported Transmog presentation stream: " .. event.stream)
