-- A dynasim defines its own plot. No Rust changes, no recompile.
-- Colours are Coherence palette ROLE names; a literal hex is rejected.

local series = {}

local function sample(name, role, kind, f)
  local pts = {}
  for i = 0, 120 do
    local t = i * 0.5
    pts[#pts + 1] = { t, f(t) }
  end
  series[#series + 1] = { name = name, color = role, kind = kind, width = 2, points = pts }
end

sample("agora-one",  "primary", "line", function(t)
  return 62 + 18 * math.sin(t / 7) + 4 * math.sin(t / 1.7)
end)

sample("mac-shell",  "info", "line", function(t)
  return 38 + 12 * math.cos(t / 9) + 3 * math.sin(t / 2.3)
end)

sample("sway-head",  "warning", "area", function(t)
  return 14 + 8 * math.abs(math.sin(t / 11))
end)

return {
  title    = "Fleet throughput",
  x_label  = "elapsed (s)",
  y_label  = "requests / s",
  x_range  = { 0, 60 },
  y_range  = "auto",     -- inferred from the data, not hand-maintained
  x_ticks  = 7,
  y_ticks  = 6,
  legend   = true,
  series   = series,
}
