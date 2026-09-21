-- One normalized FFT frame per event. Rust appends it to the widget's bounded
-- history; Lua never runs on animation frames.
local p = event.payload
if type(p) ~= "table" or type(p.bins) ~= "table" then
  error("spectral-frame-v1 requires bins")
end

local bins = canvas.array()
for _, value in ipairs(p.bins) do
  bins[#bins + 1] = math.max(0, math.min(1, tonumber(value) or 0))
end

return { kind = "spectral_frame", bins = bins }
