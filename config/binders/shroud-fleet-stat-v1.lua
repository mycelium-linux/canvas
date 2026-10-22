local p = event.payload
if type(p) ~= "table" then error("shroud-fleet-stat-v1 requires payload") end
local running = tonumber(p.running_vms) or 0
local configured = tonumber(p.configured_vms) or 0
return {kind="snapshot",widget={kind="stat",label="Running MicroVMs",value=running,state=p.error and "critical" or running < configured and "warning" or "good"},data={state=p.error and "unavailable" or "ready",source="shroud",detail=p.error or (tostring(configured) .. " configured"),provenance="live"}}
