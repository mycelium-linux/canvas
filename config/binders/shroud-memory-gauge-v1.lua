local p = event.payload
if type(p) ~= "table" then error("shroud-memory-gauge-v1 requires payload") end
local total = tonumber(p.memory_total_bytes) or 0
local available = tonumber(p.memory_available_bytes) or 0
local used = math.max(0, total - available)
local percent = total > 0 and used * 100 / total or 0
return {kind="snapshot",widget={kind="gauge",value=percent,label="Host memory · " .. string.format("%.1f GiB free", available / 1073741824)},data={state=p.error and "unavailable" or "ready",source="shroud",detail=p.error,provenance="live"}}
