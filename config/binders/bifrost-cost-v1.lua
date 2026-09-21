local p=event.payload if type(p)~="table" then error("bifrost cost requires payload") end
return {kind="snapshot",widget={kind="stat",label="Estimated provider cost",value=tonumber(p.cost_usd) or 0,unit="$",state=p.error and "critical" or "neutral"},data={state=p.error and "unavailable" or "ready",source="bifrost",detail=p.error or "List-price estimate · USD",provenance="live"}}
