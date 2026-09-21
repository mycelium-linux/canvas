local p=event.payload if type(p)~="table" or type(p.backends)~="table" then error("bifrost backends requires rows") end local rows=canvas.array()
for i=1,#p.backends do local b=p.backends[i] local up=(tonumber(b.up) or 0)==1 local open=(tonumber(b.circuit_open) or 0)==1 local name=tostring(b.backend or "—") if #name>22 then name=string.sub(name,1,19).."…" end rows[#rows+1]={name,up and "online" or "offline",tostring(b.models or 0),open and "OPEN" or "closed"} end
if #rows==0 then rows[1]={"—",tostring(p.error or "No advertised backends"),"—","—"} end
return {kind="snapshot",widget={kind="table",title="Backends",columns={"Backend","state","models","circuit"},rows=rows},data={state=p.error and "unavailable" or "ready",source="bifrost",detail=p.error,provenance="live"}}
