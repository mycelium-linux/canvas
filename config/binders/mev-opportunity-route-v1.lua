local p = event.payload
if type(p) ~= "table" or p.schema ~= "mev.opportunity.v1" or type(p.route) ~= "table" then error("mev route requires mev.opportunity.v1") end
local nodes = canvas.array()
nodes[#nodes+1] = {label="INPUT",state="neutral",detail=tostring(p.amount_in) .. " " .. tostring(p.token)}
nodes[#nodes+1] = {label=tostring(p.route[1] or "leg 1"),state="neutral",detail=tostring(p.amount_after_first) .. " intermediate units"}
nodes[#nodes+1] = {label=tostring(p.route[2] or "leg 2"),state=p.profitable and "good" or "critical",detail=tostring(p.amount_out) .. " " .. tostring(p.token)}
nodes[#nodes+1] = {label="NET",state=p.profitable and "good" or "critical",detail=tostring(p.net_profit) .. " after gas + builder"}
return {kind="snapshot",widget={kind="status_list",title="ATOMIC ROUTE",nodes=nodes},data={state="ready",source="unibus:mev-observer",detail="reconstructed · " .. tostring(p.evaluated_candidates) .. " candidate sizes evaluated",provenance="live"}}
