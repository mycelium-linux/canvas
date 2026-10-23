-- Golden parity fixture: follow the newest mesh the host enumerated.
-- Keep this scene and follow-latest.wat expressing the same SceneState.
return {
  scene_id = "follow",
  asset = assets.latest or "fallback.stl",
  camera = "iso",
  transition_ms = 0,
  fly_in = false,
}
