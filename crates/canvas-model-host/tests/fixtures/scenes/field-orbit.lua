-- Golden parity fixture: field + orbit.
-- Keep this scene and field-orbit.wat expressing the same SceneState.
return {
  scene_id = "bearing",
  asset = "sha256-bearing.stl",
  camera = "iso",
  orbit = { axis = {0, 1, 0}, degrees_per_second = 12 },
  material = { color = {0, 220, 255}, glow = 1.4, wireframe = true, opacity = 0.85 },
  field = {
    name = "thermal",
    unit = "C",
    range = {20, 180},
    colormap = "inferno",
    source = { kind = "point_heat", at = {0, 0, 0}, power = 160 },
  },
  transition_ms = 700,
  fly_in = true,
}
