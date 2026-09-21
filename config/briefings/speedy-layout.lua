-- Retained Speedy workspace compositor. Lua runs only when the active game
-- roster changes; Rust continues to own video, charts, logs, and every frame.
local payload = event.payload
if type(payload) ~= "table" or type(payload.games) ~= "table" then
  error("speedy-layout requires payload.games")
end

local prefixes = {
  ["retro-umk3"] = "umk3",
  ["retro-smk"] = "smk",
  ["retro-smb"] = "smb",
  ["retro-gb-tetris"] = "tetris",
}
local games = {}
for _, stream in ipairs(payload.games) do
  local prefix = prefixes[stream]
  if prefix ~= nil then table.insert(games, prefix) end
end
table.sort(games)

local count = #games
local cols = count <= 1 and 1 or 2
local rows = count <= 2 and 1 or 2
local placements = {}
local function place(id, x, y, width, height)
  table.insert(placements, {
    surface_id = id,
    bounds = { x = x, y = y, width = width, height = height },
    layer = 0,
  })
end

for index, prefix in ipairs(games) do
  local zero = index - 1
  local col = zero % cols
  local row = math.floor(zero / cols)
  local cell_x = col / cols
  local cell_y = row / rows
  local cell_w = 1.0 / cols
  local cell_h = 1.0 / rows
  local inset_x = 0.008
  local inset_y = 0.018
  local game_w = cell_w * 0.75
  local rail_x = cell_x + game_w + inset_x
  local rail_w = cell_w * 0.23
  local usable_h = cell_h - inset_y * 2
  place(prefix .. "-game", cell_x + inset_x, cell_y + inset_y,
        game_w - inset_x * 1.5, usable_h)
  place(prefix .. "-probabilities", rail_x, cell_y + inset_y,
        rail_w - inset_x, usable_h * 0.68)
  place(prefix .. "-rlcd", rail_x, cell_y + inset_y + usable_h * 0.70,
        rail_w - inset_x, usable_h * 0.30)
end

return {
  beat = math.floor(payload.beat or 0),
  narrate = nil,
  wait_seconds = 0,
  patches = {
    { op = "set_chrome", chrome = { title = "hidden", canvas_padding = 4, surface_gap = 4 } },
    { op = "set_layout", layout = { kind = "canvas", placements = placements } },
  },
  pre_actions = {},
  post_actions = {},
  done = false,
  message = tostring(count) .. " active Speedy game" .. (count == 1 and "" or "s"),
}
