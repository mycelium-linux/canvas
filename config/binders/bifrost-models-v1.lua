local p=event.payload
if type(p)~="table" or type(p.models)~="table" then error("bifrost models requires rows") end
local values=canvas.array()
for i=1,#p.models do
  local model=p.models[i]
  values[#values+1]={label=tostring(model.model or "Unknown"),value=tonumber(model.value) or 0}
end
if #values==0 then values[1]={label="No token observations",value=1} end
return {kind="snapshot",widget={kind="pie_chart",title="Token share by model",values=values,unit="tokens",period=tostring(p.period or "Since gateway restart")},data={state=p.error and "unavailable" or "ready",source="bifrost",detail=p.error or "Cumulative Prometheus counters",provenance="live"}}
