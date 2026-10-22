-- A dynamic simulation defining its own plot. No Rust involved.
--
-- Globals the host provides:
--   tick  : number, seconds since the view opened
--   roles : array of the palette role names this theme exposes
--
-- Contract: return a table. Colours are named by ROLE, never by RGB, so the
-- host's ThemePalette stays authoritative and a theme swap needs no edit here.

local SPAN     = 60      -- seconds visible
local RESOLUTION = 180   -- points per series

local function series(name, role, fn)
  local points = {}
  for i = 0, RESOLUTION do
    local x = (i / RESOLUTION) * SPAN
    points[#points + 1] = { x, fn(x) }
  end
  return { name = name, role = role, points = points }
end

local load_factor = 0.5 + 0.5 * math.sin(tick * 0.35)

return {
  title   = "Rack B thermal response",
  x_label = "seconds",
  y_label = "degrees C",
  x_range = { 0, SPAN },
  y_range = { 16, 44 },
  series  = {
    series("inlet", "info", function(x)
      return 21 + 1.5 * math.sin(x * 0.20 + tick)
    end),
    series("outlet", "primary", function(x)
      return 28 + 6 * load_factor + 2.0 * math.sin(x * 0.16 + tick * 0.8)
    end),
    series("cold aisle setpoint", "success", function(_)
      return 24
    end),
    series("thermal trip", "danger", function(_)
      return 41
    end),
  },
}
