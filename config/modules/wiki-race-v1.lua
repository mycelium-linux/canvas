-- Controller-owned race state; Lua-owned presentation. No navigation or I/O.
local theme, input = module.theme, module.inputs
local phase = input.status and input.status.label or "WAITING"
local age = module.now.epoch_ms - (input.observed or 0)
local fresh = input.connected == true and (input.observed or 0) > 0 and age >= -5000 and age <= 5000
fresh = fresh and type(input.raceid) == "string" and (input.revision or 0) > 0
local active = phase == "RACING" or phase == "HOLDING" or phase == "FOCUS HOLD"
local ready = phase == "READY" or phase == "STOPPED" or phase == "FINISHED"
local function clock(ms, fallback)
  if ms == nil then return fallback or "00:00.00" end
  local cs = math.floor(math.max(0, ms) / 10)
  return string.format("%02d:%02d.%02d", math.floor(cs / 6000), math.floor(cs / 100) % 60, cs % 100)
end
widget.panel({ id="hud", ink="surface", opacity=1, x=-4, y=4, width=8, height=8 })
widget.label({ id="title", text="WIKI RACE", x=-3.68, y=3.56, ink="primary", size=theme.title })
widget.badge({ id="status", text=fresh and phase or "DISCONNECTED / STALE",
  x=-1.60, y=3.56, width=2.40, height=0.65,
  ink=not fresh and "warning" or (phase == "FINISHED" and "primary" or "success") })
widget.metric({ id="start", label="START PAGE", value=input.start, x=-3.68, y=2.05, size=theme.body, value_offset=0.56 })
widget.metric({ id="target", label="TARGET PAGE", value=input.target, x=-0.65, y=2.05, size=theme.body, value_offset=0.56 })
widget.metric({ id="race", label=phase == "FINISHED" and "FINAL TIME" or "RACE CLOCK",
  value=clock(input.racems, input.raceclock), x=-3.68, y=0.43, ink="primary", size=theme.display, value_offset=0.65 })
widget.metric({ id="wall", label="WALL CLOCK", value=clock(input.wallms, input.wallclock),
  x=-0.65, y=0.43, size=theme.large, value_offset=0.65 })
widget.metric({ id="clicks", label="CLICKS", value=tostring(math.floor(input.clicks or 0)),
  x=2.30, y=0.43, size=theme.large, value_offset=0.65 })
widget.label({ id="page", text=input.page or "Waiting for controller state",
  x=-3.68, y=-1.15, ink="muted", size=theme.small })
widget.label({ id="trace", text=tostring(input.trace or "No race events yet"):sub(1,120),
  x=-3.68, y=-1.83, ink="muted", size=theme.small })
local function command(action)
  return {schema_version=1, action=action, race_id=input.raceid or "unavailable",
    expected_revision=input.revision or 0, start_page=input.start, target_page=input.target}
end
widget.button({ id="start-control", label="START", action="start", body=command("start"),
  enabled=fresh and ready, x=-3.68, y=-2.65, width=1.35, height=0.90 })
widget.button({ id="stop-control", label="STOP", action="stop", body=command("stop"),
  enabled=fresh and active, x=-2.08, y=-2.65, width=1.35, height=0.90 })
widget.label({ id="health", text=fresh and "LIVE · controller owns timing / holds" or "Controls disabled · waiting for fresh controller state",
  x=-0.16, y=-2.85, ink=fresh and "success" or "warning", size=theme.micro })
