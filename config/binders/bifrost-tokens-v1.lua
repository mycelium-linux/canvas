local p=event.payload if type(p)~="table" then error("bifrost tokens requires payload") end
return {kind="snapshot",widget={kind="stat",label="Tokens routed",value=tonumber(p.tokens) or 0,state=p.error and "critical" or "neutral"},data={state=p.error and "unavailable" or "ready",source="bifrost",detail=p.error or "Production aggregate",provenance="live"}}
