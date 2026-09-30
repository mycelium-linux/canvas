local p = event.payload
if type(p) ~= "table" or p.schema ~= "mev.opportunity.v1" then error("mev profit requires mev.opportunity.v1") end
local net = tonumber(p.net_profit) or 0
return {kind="snapshot",widget={kind="stat",label="NET · " .. tostring(p.token or "TOKEN") .. " BASE UNITS",value=net,state=net > 0 and "good" or "critical"},data={state="ready",source="unibus:mev-observer",detail="reconstructed · costs included",provenance="live"}}
