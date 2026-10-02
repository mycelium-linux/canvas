-- Canvas projection of Radioman's `SpectrumFrame` v1. Radioman owns this shape;
-- this binder only normalizes calibrated dB bins for the retained renderer.
-- Legacy unversioned `{ bins = [0..1] }` frames remain readable.
local p = event.payload
if type(p) ~= "table" or (type(p.bins_dbfs) ~= "table" and type(p.bins) ~= "table") then
  error("spectral-frame-v1 requires bins_dbfs")
end

local bins = canvas.array()
local floor_db = tonumber(p.floor_dbfs) or -110
local ceiling_db = tonumber(p.ceiling_dbfs) or -20
if floor_db >= ceiling_db then
  error("spectral-frame-v1 requires floor_db below ceiling_db")
end
for _, value in ipairs(p.bins_dbfs or p.bins) do
  local sample = tonumber(value) or floor_db
  if p.schema_version ~= nil then
    sample = (sample - floor_db) / (ceiling_db - floor_db)
  end
  bins[#bins + 1] = math.max(0, math.min(1, sample))
end

local mutation = { kind = "spectral_frame", bins = bins }
if p.schema_version ~= nil then
  if tonumber(p.schema_version) ~= 1 then
    error("spectral-frame-v1 requires schema_version 1")
  end
  if type(p.bins_dbfs) ~= "table" then
    error("radioman.spectrum.v1 requires bins_dbfs")
  end
  if type(p.session_id) ~= "string" or p.session_id == "" then
    error("spectral-frame-v1 requires session_id")
  end
  if p.generation == nil or p.sequence == nil or p.observed_at_ms == nil then
    error("spectral-frame-v1 requires generation, sequence, and observed_at_ms")
  end
  if p.center_frequency_hz == nil or p.span_hz == nil then
    error("spectral-frame-v1 requires center_frequency_hz and span_hz")
  end
  mutation.radio = {
    schema_version = 1,
    session_id = p.session_id,
    generation = tonumber(p.generation),
    sequence = tonumber(p.sequence),
    observed_at_ms = tonumber(p.observed_at_ms),
    center_frequency_hz = tonumber(p.center_frequency_hz),
    span_hz = tonumber(p.span_hz),
    floor_dbfs = floor_db,
    ceiling_dbfs = ceiling_db,
  }
end

return mutation
