local p = event.payload
if type(p) ~= "table" then error("synesthesia-routing-v1 requires a payload") end
if p.degraded then
  return {kind="snapshot",widget={kind="status_list",title="Studio routing",nodes={{label="Synesthesia",state="critical",detail=tostring(p.error or "unavailable")}}},data={state="disconnected",source="synesthesia/studio",detail=tostring(p.error or "unavailable"),provenance="live"}}
end
if type(p.routes) ~= "table" then error("synesthesia-routing-v1 requires routes") end
local nodes = canvas.array()
for i=1,math.min(#p.routes,24) do
  local route=p.routes[i]
  nodes[#nodes+1]={label=tostring(route.label or "route"),state=tostring(route.state or "good"),detail=tostring(route.detail or "active")}
end
local inventory=p.inventory or {}
local detail=string.format("%d controllers · %d instruments · %d sources · %d inserts",tonumber(inventory.controllers) or 0,tonumber(inventory.instruments) or 0,tonumber(inventory.sources) or 0,tonumber(inventory.inserts) or 0)
return {kind="snapshot",widget={kind="status_list",title=tostring(p.title or "Studio routing"),nodes=nodes},data={state=#nodes>0 and "ready" or "no_data",source="synesthesia/studio",detail=#nodes>0 and detail or "No active routes",provenance="live"}}
