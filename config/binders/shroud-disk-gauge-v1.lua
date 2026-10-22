local p = event.payload
if type(p) ~= "table" then error("shroud-disk-gauge-v1 requires payload") end
local total = tonumber(p.filesystem_size_bytes) or 0
local free = tonumber(p.filesystem_free_bytes) or 0
local used = math.max(0, total - free)
local percent = total > 0 and used * 100 / total or 0
return {kind="snapshot",widget={kind="gauge",value=percent,label="Runtime disk · " .. string.format("%.0f GiB free", free / 1073741824)},data={state=p.error and "unavailable" or "ready",source="shroud",detail=p.error,provenance="live"}}
