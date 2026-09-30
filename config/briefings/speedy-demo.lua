-- A recordable tour of the retained Speedy workspace. The briefing changes
-- presentation only: games, media decoders, and RLCD bindings remain alive.
local games = {
  { id = "umk3", name = "Ultimate Mortal Kombat Three" },
  { id = "smk", name = "Super Mario Kart" },
  { id = "smb", name = "Super Mario Brothers" },
  { id = "tetris", name = "Tetris" },
}

local function placement(id, x, y, width, height)
  return { surface_id = id, bounds = { x = x, y = y, width = width, height = height }, layer = 0 }
end

local function overview()
  local result = {}
  for index, game in ipairs(games) do
    local col = (index - 1) % 2
    local row = math.floor((index - 1) / 2)
    local x, y = col * 0.5, row * 0.5
    table.insert(result, placement(game.id .. "-game", x + 0.005, y + 0.025, 0.37, 0.45))
    table.insert(result, placement(game.id .. "-probabilities", x + 0.38, y + 0.025, 0.115, 0.30))
    table.insert(result, placement(game.id .. "-rlcd", x + 0.38, y + 0.33, 0.115, 0.145))
  end
  return result
end

local function closeup(id)
  return {
    placement(id .. "-game", 0.015, 0.025, 0.735, 0.95),
    placement(id .. "-probabilities", 0.765, 0.025, 0.22, 0.62),
    placement(id .. "-rlcd", 0.765, 0.665, 0.22, 0.31),
  }
end

local payload = event.payload
if type(payload) ~= "table" or type(payload.beat) ~= "number" then
  error("speedy-demo requires payload.beat")
end
local index = math.floor(payload.beat)
local patches = {
  { op = "set_chrome", chrome = { title = "hidden", canvas_padding = 4, surface_gap = 4 } },
}
local narrate
local done = false
if index == 0 then
  narrate = "Four emulators are live at once. Canvas retains each video surface while Speedy agents publish their decisions over Unibus."
  table.insert(patches, { op = "set_layout", layout = { kind = "canvas", placements = overview() } })
elseif index >= 1 and index <= 4 then
  local game = games[index]
  narrate = game.name .. " is enlarged without restarting its emulator. The rail shows candidate actions and the latest decision."
  table.insert(patches, { op = "set_layout", layout = { kind = "canvas", placements = closeup(game.id) } })
  table.insert(patches, { op = "set_focus", surface_id = game.id .. "-game" })
else
  narrate = "The four live sessions return to one compact workspace. Lua controls the presentation; Rust keeps rendering and transport on the hot path."
  table.insert(patches, { op = "set_layout", layout = { kind = "canvas", placements = overview() } })
  table.insert(patches, { op = "set_focus", surface_id = nil })
  done = true
end

return {
  beat = index,
  narrate = narrate,
  wait_seconds = index == 0 and 6.0 or 7.0,
  patches = patches,
  pre_actions = {},
  post_actions = {},
  done = done,
  message = done and "Speedy demo complete" or "Speedy demo",
}
