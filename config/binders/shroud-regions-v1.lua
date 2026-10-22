local p = event.payload
if type(p) ~= "table" or type(p.regions) ~= "table" then error("shroud-regions-v1 requires regions") end
local nodes = canvas.array()
for i = 1, #p.regions do
  local r = p.regions[i]
  local state = tostring(r.state or "unknown")
  local level = state == "selected" and "good" or state == "available" and "neutral" or state == "unavailable" and "critical" or "warning"
  local detail = state == "selected" and "Active region" or state == "not_configured" and "Not configured" or state
  nodes[#nodes + 1] = {label=tostring(r.label or r.id or "region"),state=level,detail=detail}
end
return {kind="snapshot",widget={kind="status_list",title="Region · " .. tostring(p.selected_region or "unknown"),nodes=nodes},data={state="ready",source="shroud",provenance="live"}}
