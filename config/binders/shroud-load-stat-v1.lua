local p = event.payload
if type(p) ~= "table" then error("shroud-load-stat-v1 requires payload") end
local load = tonumber(p.load_1m) or 0
return {kind="snapshot",widget={kind="stat",label="1 minute load",value=load,state=p.error and "critical" or load >= 8 and "critical" or load >= 4 and "warning" or "good"},data={state=p.error and "unavailable" or "ready",source="shroud",detail=p.error or tostring(p.region or "unknown"),provenance="live"}}
