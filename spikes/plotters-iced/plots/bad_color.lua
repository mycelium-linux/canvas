-- A script that tries to pick its own colour outside the palette.
-- This MUST be rejected, or Canvas loses the ability to re-skin its charts.
return {
  title = "off-palette",
  series = { { name = "rogue", color = "#ff00ff", points = { { 0, 0 }, { 1, 1 } } } },
}
