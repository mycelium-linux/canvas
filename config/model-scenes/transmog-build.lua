-- Follow a transmog build: hold the newest step it has produced.
--
-- transmog writes step-0.stl, step-1.stl ... into its --stream directory as it
-- computes them, and never rewrites one. The host enumerates that directory in
-- build order and hands it over as `assets`; choosing which step to show is
-- this file's business, not the renderer's.
--
-- Hot-reloaded: edit this and the view changes on the next scan, no restart.
local steps = assets.steps or {}
local newest = assets.latest

-- The sandbox is StdLib::NONE: no math, no string, no table libraries. Only
-- language operators are available, so clamp by hand.
local depth = #steps
if depth > 20 then
  depth = 20
end

return {
  scene_id = "transmog-build",
  -- Before the first step lands there is nothing to show, so fall back to the
  -- Benchy. A missing asset would fail the bake and leave the surface black.
  asset = newest or "sha256-6ab57f1c3f8e86bc3cbd302c6fa6270acf06277c6335454e922419c25d42e97e.stl",
  -- transmog emits Z-up like most CAD; this is asset-local metadata, not a
  -- Canvas coordinate-system claim.
  model_rotation_degrees = { -90, 0, 0 },
  camera = "iso",
  orbit = {
    axis = { 0, 1, 0 },
    -- Slow enough to read the geometry while it changes underneath.
    degrees_per_second = 12,
  },
  material = {
    color = nil,
    -- Lift the glow as the part gains steps, so a long build reads as
    -- progress rather than as a static object that happens to differ.
    glow = 1.1 + depth * 0.02,
    wireframe = false,
    opacity = 0.9,
  },
  -- Paint is engineering evidence, not decoration. Transmog's stream does not
  -- yet publish a sampled closed-form result, so use the neutral material.
  transition_ms = 260,
  -- Each step is a cut, not a camera re-fly: re-flying on every step would
  -- spend the whole build swooping and never settle.
  fly_in = false,
}
