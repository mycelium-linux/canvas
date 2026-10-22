-- Normalize room-scale tracks into Canvas's bounded spatial snapshot.
-- The producer owns tracking, association, and TTL; Rust owns presentation.
local p = event.payload
if type(p) ~= "table" or type(p.tracks) ~= "table" or type(p.bounds) ~= "table" then
  error("spatial-tracks-v1 requires tracks and bounds")
end

local tracks = canvas.array()
for _, track in ipairs(p.tracks) do
  if type(track) == "table" and type(track.position) == "table" then
    local trail = canvas.array()
    for _, point in ipairs(track.trail or {}) do
      if type(point) == "table" then
        trail[#trail + 1] = { x = tonumber(point.x) or 0, z = tonumber(point.z) or 0 }
      end
    end
    tracks[#tracks + 1] = {
      id = math.max(0, math.floor(tonumber(track.id) or 0)),
      position = {
        x = tonumber(track.position.x) or 0,
        z = tonumber(track.position.z) or 0,
      },
      trail = trail,
      confidence = math.max(0, math.min(1, tonumber(track.confidence) or 0)),
      motion = math.max(0, math.min(100, tonumber(track.motion) or 0)),
    }
  end
end

return {
  kind = "snapshot",
  widget = {
    kind = "spatial_tracks",
    title = tostring(p.title or "Spatial tracks"),
    tracks = tracks,
    bounds = {
      min_x = tonumber(p.bounds.min_x) or 0,
      max_x = tonumber(p.bounds.max_x) or 1,
      min_z = tonumber(p.bounds.min_z) or 0,
      max_z = tonumber(p.bounds.max_z) or 1,
    },
  },
  data = {
    state = #tracks > 0 and (p.state or "ready") or "no_data",
    source = tostring(p.source or event.source or "spatial"),
    detail = p.detail and tostring(p.detail) or nil,
    provenance = "live",
  },
}
