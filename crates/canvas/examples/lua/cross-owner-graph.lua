-- Cross-owner activity graph: combines bifrost and elmers inputs into one scene.
--
-- Producer-agnostic: both owners feed typed GraphSnapshot values, and the
-- module renders them as two adjacent columns with a bridge node that marks
-- where the two systems cross. Nothing here reads a clock or a socket.

local bifrost = module.inputs.bifrost
local elmers = module.inputs.elmers

local RAMP = { "\u{00B7}", "\u{2591}", "\u{2592}", "\u{2593}", "\u{2588}" }
local function mark(heat)
  local clamped = math.max(0, math.min(1, heat))
  return RAMP[1 + math.floor(clamped * (#RAMP - 1) + 0.5)]
end

local FLAG = { critical = "!", warning = "?", good = "", neutral = "" }

local SOURCES = {
  { id = "bifrost", data = bifrost, x = -0.25 },
  { id = "elmers", data = elmers, x = 0.25 },
}

local COLUMN_W, ROW_H = 0.5, 0.14

canvas.group { id = "cross" }

local placed = {}
for source_idx, src in ipairs(SOURCES) do
  local columns, order = {}, {}
  if src.data and src.data.nodes then
    for _, node in ipairs(src.data.nodes) do
      local key = node.group or "node"
      if not columns[key] then
        columns[key] = {}
        order[#order + 1] = key
      end
      columns[key][#columns[key] + 1] = node
    end
  end
  local column_x = src.x
  for column_index, key in ipairs(order) do
    local x = column_x + (column_index - 1.5) * COLUMN_W * 0.6
    canvas.text {
      id = src.id .. "-group-" .. key,
      parent = "cross",
      text = src.id .. "/" .. key,
      translation = { x, 0.16, 0 },
    }
    for row, node in ipairs(columns[key]) do
      local y = -(row - 1) * ROW_H
      placed[src.id .. "-" .. node.id] = { x = x, y = y }
      local heat = math.max(0, math.min(1, node.heat or 0))
      canvas.text {
        id = src.id .. "-" .. node.id,
        parent = "cross",
        text = mark(heat) .. " " .. node.label .. (FLAG[node.level] or ""),
        scale = { 1 + 0.25 * heat, 1 + 0.25 * heat, 1 },
        translation = { x, y, 0 },
      }
    end
  end
end

-- Bridge: if both sources have at least one node, draw a connection.
local b_first = next(placed)
local e_first = nil
for k in pairs(placed) do
  if not k:match("^bifrost%-") and not e_first then
    e_first = k
  end
end
if b_first and e_first then
  local bx, by = placed[b_first].x, placed[b_first].y
  local ex, ey = placed[e_first].x, placed[e_first].y
  local mid_x = (bx + ex) / 2
  canvas.text {
    id = "bridge",
    parent = "cross",
    text = "bridge",
    translation = { mid_x, (by + ey) / 2, 0 },
  }
end
