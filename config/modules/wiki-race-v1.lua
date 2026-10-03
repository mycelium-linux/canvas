-- Presentation-only wiki race surface. Surf owns navigation and timing; this
-- module composes the supplied snapshot from bounded Canvas primitives.
local theme = module.theme
local input = module.inputs

local function size(px)
  return px / (theme.body * 1.25)
end

local function label(id, value, x, y, role, scale)
  canvas.text({
    id = id,
    text = tostring(value or "—"),
    ink = role,
    translation = { x, y, 0.2 },
    scale = { size(scale), size(scale), 1 },
  })
end

canvas.rect({
  id = "hud",
  ink = "text",
  opacity = 0.055,
  radius = theme.radius,
  translation = { -3.84, 3.68, 0 },
  scale = { 7.68, 1.42, 1 },
})
canvas.rect({
  id = "trace-panel",
  ink = "text",
  opacity = 0.04,
  radius = theme.radius,
  translation = { 1.72, 1.98, 0 },
  scale = { 2.10, 5.56, 1 },
})

label("title", "WIKI RACE", -3.58, 3.40, "primary", theme.title)
label("status", input.status and input.status.label or "READY", -1.85, 3.40, "success", theme.small)
label("start-caption", "START", -3.58, 2.96, "muted", theme.micro)
label("start", input.start, -3.58, 2.66, "text", theme.body)
label("target-caption", "TARGET", -0.76, 2.96, "muted", theme.micro)
label("target", input.target, -0.76, 2.66, "text", theme.body)
label("race-caption", "RACE", 1.94, 3.36, "muted", theme.micro)
label("race", input.raceclock, 1.94, 2.83, "primary", theme.display)
label("wall-caption", "WALL", 3.03, 3.36, "muted", theme.micro)
label("wall", input.wallclock, 3.03, 2.83, "text", theme.large)
label("clicks", string.format("%02d CLICKS", math.floor(input.clicks or 0)), 2.98, 2.48, "muted", theme.small)

label("trace-title", "SURF TRACE", 1.94, 1.66, "primary", theme.large)
local line = 0
for entry in tostring(input.trace or "waiting for controller"):gmatch("([^|]+)") do
  if line >= 12 then break end
  label("trace-" .. line, entry, 1.94, 1.22 - line * 0.42, line == 0 and "text" or "muted", theme.small)
  line = line + 1
end
