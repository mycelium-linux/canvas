-- Presentation-only wiki race surface. Surf owns navigation and timing; this
-- module composes the supplied snapshot from bounded Canvas primitives.
local theme = module.theme
local input = module.inputs

widget.panel({
  id = "hud",
  ink = "text",
  opacity = 0.055,
  x = -3.84, y = 3.68,
  width = 7.68, height = 1.42,
})
widget.panel({
  id = "trace-panel",
  ink = "text",
  opacity = 0.04,
  x = 1.72, y = 1.98,
  width = 2.10, height = 5.56,
})

widget.label({ id = "title", text = "WIKI RACE", x = -3.58, y = 3.40, ink = "primary", size = theme.title })
widget.badge({
  id = "status", text = input.status and input.status.label or "READY",
  x = -1.85, y = 3.40, width = 0.86, height = 0.28,
  ink = input.status and input.status.level == "critical" and "danger" or "success",
})
widget.metric({ id = "start", label = "START", value = input.start, x = -3.58, y = 2.96, size = theme.body, value_offset = 0.30 })
widget.metric({ id = "target", label = "TARGET", value = input.target, x = -0.76, y = 2.96, size = theme.body, value_offset = 0.30 })
widget.metric({ id = "race", label = "RACE", value = input.raceclock, x = 1.94, y = 3.36, ink = "primary", size = theme.display, value_offset = 0.53 })
widget.metric({
  id = "wall", label = "WALL", value = input.wallclock, x = 3.03, y = 3.36,
  size = theme.large, value_offset = 0.53,
  detail = string.format("%02d CLICKS", math.floor(input.clicks or 0)), detail_offset = 0.88,
})

widget.label({ id = "trace-title", text = "SURF TRACE", x = 1.94, y = 1.66, ink = "primary", size = theme.large })
local line = 0
for entry in tostring(input.trace or "waiting for controller"):gmatch("([^|]+)") do
  if line >= 12 then break end
  widget.label({
    id = "trace-" .. line, text = entry, x = 1.94, y = 1.22 - line * 0.42,
    ink = line == 0 and "text" or "muted", size = theme.small,
  })
  line = line + 1
end
