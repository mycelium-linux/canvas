-- Official CreativeTools 3DBenchy, rendered as a theme-native hologram.
-- Source: https://github.com/CreativeTools/3DBenchy
-- This file is hot-reloaded; Rust retains the result and owns every frame.
return {
  scene_id = "benchy",
  asset = "sha256-6ab57f1c3f8e86bc3cbd302c6fa6270acf06277c6335454e922419c25d42e97e.stl",
  -- 3DBenchy is authored Z-up. This is asset-local metadata, not a Canvas
  -- coordinate-system assumption.
  model_rotation_degrees = { -90, 0, 0 },
  camera = "iso",
  orbit = {
    axis = { 0, 1, 0 },
    degrees_per_second = 18,
  },
  material = {
    color = nil,
    glow = 1.35,
    wireframe = false,
    opacity = 0.88,
  },
  transition_ms = 950,
  fly_in = true,
}
