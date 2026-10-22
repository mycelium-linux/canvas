local p = event.payload
if type(p) ~= "table" then error("synesthesia-quality-v1 requires a payload") end
if p.degraded then
  return {kind="snapshot",widget={kind="status_list",title="Signal quality",nodes={{label="Telemetry",state="critical",detail=tostring(p.error or "unavailable")}}},data={state="disconnected",source="synesthesia/daw",detail=tostring(p.error or "unavailable"),provenance="live"}}
end
if type(p.metrics) ~= "table" then error("synesthesia-quality-v1 requires metrics") end
local metrics=canvas.array()
for i=1,math.min(#p.metrics,8) do
  local metric=p.metrics[i]
  metrics[#metrics+1]={label=tostring(metric.label or "signal"),value=tonumber(metric.value) or 0,max=tonumber(metric.max) or 60,target=tonumber(metric.target),state=tostring(metric.state or "good")}
end
return {kind="snapshot",widget={kind="metric_board",title=tostring(p.title or "Signal headroom"),style="bullet",metrics=metrics,unit=tostring(p.unit or "dB")},data={state=#metrics>0 and "ready" or "no_data",source="synesthesia/daw",detail=tostring(p.detail or "Live peaks"),provenance="live"}}
