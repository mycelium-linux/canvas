-- Follow a transmog build: hold the newest step it has produced.
--
-- Canvas publishes immutable STL, GLB, or scene-manifest generations into the
-- model cache. The host enumerates them in build order and hands them over as
-- `assets`; choosing which generation to show stays scene policy.
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
  -- GLB's coordinate convention is already owned by the asset.
  model_rotation_degrees = { 0, 0, 0 },
  camera = "iso",
  material = {
    -- The current DesignDocument names PETG and no finish. Present that as a
    -- neutral blue-grey polymer instead of inheriting a decorative theme hue.
    color = { 194, 199, 207 },
    -- A real lit surface, not the old decorative hologram treatment.
    glow = 0.0,
    wireframe = false,
    opacity = 1.0,
  },
  -- Paint is engineering evidence, not decoration. Transmog's stream does not
  -- yet publish a sampled closed-form result, so use the neutral material.
  transition_ms = 260,
  -- Each step is a cut, not a camera re-fly: re-flying on every step would
  -- spend the whole build swooping and never settle.
  fly_in = false,
}
