local p=event.payload if type(p)~="table" or type(p.nodes)~="table" then error("bifrost mesh requires rows") end local rows=canvas.array()
local function gib(v) if not tonumber(v) then return "—" end return string.format("%.1f GiB",tonumber(v)/1073741824) end
for i=1,#p.nodes do local n=p.nodes[i] local used=gib(n.memory_used_bytes) local total=gib(n.memory_bytes) local utilization=tonumber(n.utilization_pct) local power=tonumber(n.power_watts) rows[#rows+1]={tostring(n.node or "—"),(tonumber(n.up) or 0)==1 and "online" or "offline",utilization and string.format("%.0f%%",utilization) or "—",used.." / "..total,power and string.format("%.0f W",power) or "—"} end
if #rows==0 then rows[1]={"—",tostring(p.error or "No mesh nodes reported"),"—","—","—"} end
return {kind="snapshot",widget={kind="table",title="Accelerator mesh",columns={"Node","state","GPU","VRAM","power"},rows=rows},data={state=p.error and "unavailable" or "ready",source="bifrost",detail=p.error,provenance="live"}}
