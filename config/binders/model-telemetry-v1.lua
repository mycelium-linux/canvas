local p=event.payload
if type(p)~="table" or type(p.annotations)~="table" then error("model-telemetry-v1 requires annotations") end
local annotations=canvas.array()
for _,item in ipairs(p.annotations) do if type(item)=="table" and type(item.position)=="table" then annotations[#annotations+1]={id=tostring(item.id or ""),label=tostring(item.label or item.id or ""),position={tonumber(item.position[1]) or 0,tonumber(item.position[2]) or 0,tonumber(item.position[3]) or 0},value=tostring(item.value or ""),state=item.state and tostring(item.state) or nil} end end
return {kind="snapshot",widget={kind="model_telemetry",title=tostring(p.title or "Model telemetry"),model=tostring(p.model or "model"),view=tostring(p.view or "isometric"),annotations=annotations,exploded=math.max(0,math.min(1,tonumber(p.exploded) or 0))},data={state=p.state or "ready",source=tostring(p.source or event.source or "model"),provenance="live"}}
