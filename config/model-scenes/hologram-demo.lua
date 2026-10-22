-- Evaluated only when loaded or changed. Rust retains this state and owns the
-- WGPU animation/render loop; no Lua runs per frame.
return {
  scene_id = "hologram-demo",
  asset = "sha256-demo-part.stl",
  model_rotation_degrees = { 0, 0, 0 },
  camera = "iso",
  orbit = {
    axis = { 0, 1, 0 },
    degrees_per_second = 9,
  },
  material = {
    -- nil inherits the active Canvas theme primary colour.
    color = nil,
    glow = 1.35,
    wireframe = true,
    opacity = 0.88,
  },
  transition_ms = 850,
  fly_in = true,
}
