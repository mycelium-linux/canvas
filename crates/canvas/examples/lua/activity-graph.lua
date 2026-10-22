-- A live firing view: what is doing something, right now.
--
-- Ported from the reasoning in elmers `src/serve/brain.rs` and its dashboard. The
-- module draws; the host owns the topology and how recently each part of it did
-- something. Nothing here reads a clock or a socket, and it keeps no state
-- between invocations — each runs in a fresh VM, so heat arrives already decayed
-- in `module.inputs.graph`.
--
-- Producer-agnostic on purpose. Elmers fills it with workflows and capabilities;
-- the MCP bridge fills it with calling systems, connectors and tools. This never
-- learns which, which is the point of giving the graph a typed shape rather than
-- a view per source.
--
-- Heat is drawn with glyph and scale rather than colour: `canvas.text` takes only
-- id, parent, text, translation, rotation and scale, so a module cannot tint
-- anything. `module.theme` is still read for the mode label, and the day text
-- nodes accept a colour this wants `level` mapped onto the semantic tokens.

local graph = module.inputs.graph

-- Coarse enough to read at a glance, fine enough that a decaying node visibly
-- steps down rather than blinking off.
local RAMP = { "\u{00B7}", "\u{2591}", "\u{2592}", "\u{2593}", "\u{2588}" }

local function mark(heat)
  local clamped = math.max(0, math.min(1, heat))
  return RAMP[1 + math.floor(clamped * (#RAMP - 1) + 0.5)]
end

-- Without colour, the outcome has to be legible in the label itself. A run that
-- failed is the thing you are looking for; it must not read the same as a quiet
-- one that succeeded an hour ago.
local FLAG = { critical = "!", warning = "?", good = "", neutral = "" }

-- Grouping drives layout: one column per family, in first-seen order, so a graph
-- fed by two owners reads as two bands rather than one cloud.
local columns, order = {}, {}
for _, node in ipairs(graph.nodes) do
  local key = node.group or "node"
  if not columns[key] then
    columns[key] = {}
    order[#order + 1] = key
  end
  local column = columns[key]
  column[#column + 1] = node
end

local COLUMN_W, ROW_H = 0.5, 0.14

canvas.group { id = "activity" }

local placed = {}
for column_index, key in ipairs(order) do
  local x = (column_index - 1) * COLUMN_W
  canvas.text {
    id = "group-" .. key,
    parent = "activity",
    text = key,
    translation = { x, 0.16, 0 },
  }
  for row, node in ipairs(columns[key]) do
    local y = -(row - 1) * ROW_H
    placed[node.id] = { x = x, y = y }
    local heat = math.max(0, math.min(1, node.heat))
    canvas.text {
      id = "n-" .. node.id,
      parent = "activity",
      text = mark(heat) .. " " .. node.label .. (FLAG[node.level] or ""),
      -- A node that just fired is briefly larger. The shape of the system stays
      -- readable when nothing is happening, instead of the view going blank.
      scale = { 1 + 0.25 * heat, 1 + 0.25 * heat, 1 },
      translation = { x, y, 0 },
    }
  end
end

-- Both ends of a call light, and the edge between them carries the pulse. That is
-- what turns a fan-out into one body with a ring around it rather than a scatter
-- of unrelated dots.
for index, edge in ipairs(graph.edges) do
  local from, to = placed[edge.from], placed[edge.to]
  if from and to and edge.heat > 0.02 then
    canvas.text {
      id = "e-" .. index,
      parent = "activity",
      text = mark(edge.heat),
      translation = { (from.x + to.x) / 2, (from.y + to.y) / 2, 0 },
    }
  end
end

-- A view showing nothing when nothing has happened is indistinguishable from a
-- broken one, which is the same reason the stream sends a ping.
if #graph.nodes == 0 then
  canvas.text {
    id = "empty",
    parent = "activity",
    text = "no topology yet \u{00B7} " .. module.theme.mode,
    translation = { 0, 0, 0 },
  }
end
