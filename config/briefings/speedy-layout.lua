-- Retained Speedy compositor: one game in the spotlight, the rest along the
-- bottom, rotating on the beat. Lua runs only when a beat arrives; Rust keeps
-- owning video, widgets and every frame.
--
-- A four-quadrant grid gave every game the same quarter of the wall and left
-- the decision log too narrow to read a line in. Spotlighting one at a time
-- buys that game enough width for a real RLCD panel, and the others stay
-- visible as thumbnails rather than disappearing.
local payload = event.payload
if type(payload) ~= "table" or type(payload.games) ~= "table" then
  error("speedy-layout requires payload.games")
end

-- Stream id to surface prefix. A stream with no prefix here is not part of
-- this board and is skipped rather than guessed at.
local prefixes = {
  ["retro-umk3"] = "umk3",
  ["retro-smk"] = "smk",
  ["retro-smb"] = "smb",
  ["retro-gb-tetris"] = "tetris",
  ["retro-2048"] = "retro2048",
  ["retro-freedoom"] = "freedoom",
}

local games = {}
for _, stream in ipairs(payload.games) do
  local prefix = prefixes[stream]
  if prefix ~= nil then table.insert(games, prefix) end
end
-- Sorted so every screen composes the same board from the same roster: the
-- order must not depend on map iteration.
table.sort(games)

local count = #games
local placements = {}
local function place(id, x, y, width, height)
  table.insert(placements, {
    surface_id = id,
    bounds = { x = x, y = y, width = width, height = height },
    layer = 0,
  })
end

local EDGE = 0.004
local GAP = 0.006
local SPAN = 1.0 - EDGE * 2

if count == 0 then
  -- Nothing streaming: emit an empty arrangement rather than a layout
  -- referencing surfaces that may not be on this board.
  return {
    beat = math.floor(payload.beat or 0),
    narrate = nil,
    wait_seconds = 0,
    patches = {
      { op = "set_layout", layout = { kind = "canvas", placements = {} } },
    },
    pre_actions = {},
    post_actions = {},
    done = false,
    message = "no Speedy game is streaming",
  }
end

-- The beat picks the spotlight, so rotation needs no state here: the same beat
-- always composes the same board, which is what lets three screens agree.
local hero_index = (math.floor(payload.beat or 0) % count) + 1
local hero = games[hero_index]

if count == 1 then
  -- Alone in the spotlight, with the whole wall to itself.
  place(hero .. "-game", EDGE, EDGE, SPAN * 0.68, SPAN)
  place(hero .. "-decision", EDGE + SPAN * 0.68 + GAP, EDGE, SPAN * 0.32 - GAP, SPAN)
else
  local STRIP_H = 0.20
  local hero_h = SPAN - STRIP_H - GAP
  -- The spotlight keeps roughly two thirds for video and one third for the
  -- decision panel, which is the narrowest it can be and still fit a line.
  local hero_w = SPAN * 0.66
  place(hero .. "-game", EDGE, EDGE, hero_w, hero_h)
  place(hero .. "-decision", EDGE + hero_w + GAP, EDGE, SPAN - hero_w - GAP, hero_h)

  -- Everything else, in roster order starting after the spotlight so a game
  -- keeps its place in the strip as the spotlight moves along.
  local others = {}
  for offset = 1, count - 1 do
    table.insert(others, games[((hero_index - 1 + offset) % count) + 1])
  end

  local slots = #others
  local slot_w = (SPAN - GAP * (slots - 1)) / slots
  local strip_y = EDGE + hero_h + GAP
  for index, prefix in ipairs(others) do
    place(prefix .. "-game", EDGE + (index - 1) * (slot_w + GAP), strip_y, slot_w, STRIP_H)
  end
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
  message = hero .. " in the spotlight, " .. tostring(count - 1) .. " waiting",
}
