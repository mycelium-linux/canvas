local p = event.payload
if type(p) ~= "table" or p.schema ~= "mev.opportunity.v1" then error("mev profit requires mev.opportunity.v1") end
local net = tonumber(p.net_profit) or 0
local weth = p.token == "WETH"
return {kind="snapshot",widget={kind="stat",label="NET OPPORTUNITY",value=weth and net / 1e18 or net,unit=weth and "WETH" or "base units",state=net > 0 and "good" or "critical"},data={state="ready",source="unibus:mev-observer",detail="reconstructed · gas + builder costs included",provenance="live"}}
