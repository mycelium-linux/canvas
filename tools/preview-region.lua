-- Offscreen preview of a chrome region, for design work.
--
--   luajit tools/preview-region.lua <source.lua> <width> <height> [out.svg]
--
-- It runs the region's real source against the real widget library and the same
-- scene API the compositor offers, then writes the collected nodes as SVG. That
-- is enough to judge spacing, rhythm and colour without launching a shell; it is
-- not a renderer, so text is estimated and effect layers (glass, shadows) are
-- sketched rather than blurred.

local source_path, width, height, out_path = ...
width, height = tonumber(width) or 900, tonumber(height) or 200
out_path = out_path or "/tmp/region.svg"

-- LuaJIT is 5.1: the compositor's VM has the 5.4 `utf8` library, this harness
-- needs enough of it to run a source file unchanged.
if not utf8 then
  utf8 = {
    len = function(text) return #text end,
    char = function(code) return string.char(code % 256) end,
    codepoint = function(text, from) return text:byte(from or 1) end,
    offset = function(text, n) return (n <= #text + 1) and n or nil end,
  }
end

local nodes = {}
local function push(node) nodes[#nodes + 1] = node end

canvas = {}
local function node(spec, content)
  push {
    id = spec.id,
    parent = spec.parent,
    translation = spec.translation or { 0, 0, 0 },
    scale = spec.scale or { 1, 1, 1 },
    content = content,
  }
end
canvas.group = function(spec) node(spec, { kind = "group" }) end
canvas.rect = function(spec)
  node(spec, {
    kind = "rect",
    ink = spec.ink,
    rgb = spec.rgb,
    opacity = spec.opacity or 1,
    radius = spec.radius or 0,
  })
end
canvas.text = function(spec)
  node(spec, { kind = "text", text = spec.text, ink = spec.ink, size = (spec.scale or { 1 })[1] })
end
canvas.image = function(spec) node(spec, { kind = "image", asset = spec.asset }) end
canvas.mesh = function(spec) node(spec, { kind = "image", asset = spec.asset }) end

-- The shell's dark palette, near enough to judge a composition by.
local theme = {
  mode = "dark",
  accent = "brand",
  background = { 12, 13, 16 },
  surface = { 24, 26, 31 },
  text = { 238, 240, 245 },
  muted = { 150, 155, 168 },
  primary = { 108, 168, 255 },
  success = { 110, 200, 140 },
  warning = { 232, 178, 90 },
  danger = { 226, 110, 110 },
  border = { 52, 56, 66 },
  radius = 12,
  title = 26,
  body = 15,
  small = 11,
  micro = 9,
}

local inputs = {}
do
  local file = io.open((os.getenv("PREVIEW_INPUTS") or ""), "r")
  if file then
    local chunk = file:read("*a")
    file:close()
    inputs = loadstring("return " .. chunk)()
  end
end

module = {
  id = "preview",
  theme = theme,
  inputs = inputs,
  now_ms = tonumber(os.getenv("PREVIEW_NOW_MS")) or 900,
  viewport = { width = width, height = height },
  select = function() end,
  animate = function() end,
}
widget = {}
-- The host supplies these; a preview only needs their shape.
widget.button = function(spec) return spec end
widget.hit = function(spec) return spec end

local function run(path)
  local file = assert(io.open(path, "r"), "cannot read " .. path)
  local text = file:read("*a")
  file:close()
  assert(loadstring(text, "@" .. path))()
end

-- The library the compositor would load: the runtime copy if a shell has run,
-- otherwise the source in this checkout.
local library = os.getenv("PREVIEW_WIDGETS")
  or (io.open(os.getenv("HOME") .. "/Library/Application Support/Dock/canvas/lua/widgets.lua", "r")
    and (os.getenv("HOME") .. "/Library/Application Support/Dock/canvas/lua/widgets.lua"))
  or "config/lua/widgets.lua"
run(library)
run(source_path)

local function colour(value, ink, opacity)
  local source = value
  if not source then
    source = theme[ink or "text"]
  end
  local scale = 1
  for _, part in ipairs(source) do
    if part > 1 then scale = 255; break end
  end
  local r, g, b = math.floor(source[1] * scale), math.floor(source[2] * scale), math.floor(source[3] * scale)
  local a = (source[4] and (source[4] <= 1 and source[4] or source[4] / 255)) or 1
  return string.format("rgba(%d,%d,%d,%.3f)", r, g, b, a * (opacity or 1))
end

local function rgb(value)
  if type(value) == "table" then return value end
  if type(value) ~= "string" then return nil end
  local hex = value:gsub("#", "")
  return {
    tonumber(hex:sub(1, 2), 16),
    tonumber(hex:sub(3, 4), 16),
    tonumber(hex:sub(5, 6), 16),
  }
end

local by_id = {}
for _, item in ipairs(nodes) do by_id[item.id] = item end
local function offset(item)
  if not item.parent then return 0, 0 end
  local parent = by_id[item.parent]
  if not parent then return 0, 0 end
  local px, py = offset(parent)
  return px + parent.translation[1], py + parent.translation[2]
end

local function to_px(x, y)
  return (x + 4) / 8 * width, (4 - y) / 8 * height
end

local unit = theme.body * 1.25
local out = {
  string.format('<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d">', width, height),
  string.format('<rect width="%d" height="%d" fill="%s"/>', width, height,
    colour(rgb("#1a1c22"))),
}
for _, item in ipairs(nodes) do
  local ox, oy = offset(item)
  local x, y = to_px(item.translation[1] + ox, item.translation[2] + oy)
  local content = item.content
  if content.kind == "rect" then
    local w = math.abs(item.scale[1]) / 8 * width
    local h = math.abs(item.scale[2]) / 8 * height
    local fill = colour(rgb(content.rgb), content.ink, content.opacity)
    out[#out + 1] = string.format(
      '<rect x="%.2f" y="%.2f" width="%.2f" height="%.2f" rx="%.2f" fill="%s"/>',
      x, y, w, h, math.min(content.radius, w / 2), fill)
  elseif content.kind == "text" then
    local size = content.size * unit
    out[#out + 1] = string.format(
      '<text x="%.2f" y="%.2f" font-family="Helvetica" font-size="%.2f" fill="%s">%s</text>',
      x, y + size, size, colour(rgb(content.rgb), content.ink, 1),
      (tostring(content.text):gsub("&", "&amp;"):gsub("<", "&lt;")))
  elseif content.kind == "image" then
    out[#out + 1] = string.format(
      '<rect x="%.2f" y="%.2f" width="%.2f" height="%.2f" fill="none" stroke="#556" stroke-dasharray="3 3"/>',
      x, y, math.abs(item.scale[1]) / 8 * width, math.abs(item.scale[2]) / 8 * height)
  end
end
out[#out + 1] = "</svg>"
local file = assert(io.open(out_path, "w"))
file:write(table.concat(out, "\n"))
file:close()
print(string.format("%d nodes -> %s", #nodes, out_path))

-- The same nodes as data, for tools/preview-render.py: an SVG would be prettier
-- but the rasteriser on this machine is not, and a preview you cannot look at is
-- not a preview.
local function esc(text)
  return (tostring(text):gsub('\\', '\\\\'):gsub('"', '\\"'):gsub("\n", " "))
end
local json = { string.format('{"width":%d,"height":%d,"nodes":[', width, height) }
for index, item in ipairs(nodes) do
  local ox, oy = offset(item)
  local x, y = to_px(item.translation[1] + ox, item.translation[2] + oy)
  local content = item.content
  local colour_value = content.rgb and rgb(content.rgb)
  json[#json + 1] = string.format(
    '%s{"kind":"%s","x":%.2f,"y":%.2f,"w":%.2f,"h":%.2f,"radius":%.2f,"ink":"%s","rgb":[%s,%s,%s],"opacity":%.3f,"text":"%s","size":%.2f}',
    index > 1 and "," or "", content.kind, x, y,
    math.abs(item.scale[1]) / 8 * width, math.abs(item.scale[2]) / 8 * height,
    content.radius or 0, content.ink or "text",
    colour_value and colour_value[1] or "null",
    colour_value and colour_value[2] or "null",
    colour_value and colour_value[3] or "null",
    content.opacity or 1, esc(content.text or ""), (content.size or 0) * unit)
end
json[#json + 1] = "]}"
local data = assert(io.open(out_path .. ".json", "w"))
data:write(table.concat(json))
data:close()
