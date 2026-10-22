local p = event.payload
if type(p) ~= "table" then error("synesthesia-mix-field-v1 requires a payload") end
if p.degraded then
  return {kind="snapshot",widget={kind="mix_field",title="Studio mix field",surface="unavailable",strips={{index=1,cc=0,label="Telemetry unavailable",gain=0,active=false,peak=0,stereo_position=0,left_peak=0,right_peak=0,bands=canvas.array()}}},data={state="disconnected",source="synesthesia/mix-field",detail=tostring(p.error or "unavailable"),provenance="live"}}
end
if type(p.strips) ~= "table" then error("synesthesia-mix-field-v1 requires strips") end
local strips = canvas.array()
for i, strip in ipairs(p.strips) do
  if i > 8 then break end
  local bands = canvas.array()
  if type(strip.bands) == "table" then
    for b, value in ipairs(strip.bands) do if b <= 64 then bands[b] = tonumber(value) or 0 end end
  end
  strips[#strips + 1] = {index=math.floor(tonumber(strip.index) or i),cc=math.floor(tonumber(strip.cc) or 0),label=tostring(strip.label or ("Strip " .. i)),gain=tonumber(strip.gain) or 0,active=strip.active==true,peak=tonumber(strip.peak) or 0,stereo_position=tonumber(strip.stereo_position) or 0,left_peak=tonumber(strip.left_peak) or 0,right_peak=tonumber(strip.right_peak) or 0,bands=bands}
end
if #strips == 0 then error("synesthesia-mix-field-v1 requires at least one strip") end
return {kind="snapshot",widget={kind="mix_field",title=tostring(p.title or "Studio mix field"),surface=tostring(p.surface or "controller"),strips=strips},data={state="ready",source="synesthesia/mix-field",detail="Live controller strips",provenance="live"}}
