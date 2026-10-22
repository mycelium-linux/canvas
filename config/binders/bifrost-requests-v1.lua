local p=event.payload if type(p)~="table" then error("bifrost requests requires payload") end
return {kind="snapshot",widget={kind="stat",label="Production requests",value=tonumber(p.requests) or 0,state=p.error and "critical" or "good"},data={state=p.error and "unavailable" or "ready",source="bifrost",detail=p.error or "Lifetime routed",provenance="live"}}
