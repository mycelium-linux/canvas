local p = event.payload
if type(p) ~= "table" or type(p.recent_deliveries) ~= "table" then error("fab-deliveries-v1 requires recent_deliveries") end
local entries = canvas.array()
for i = 1, #p.recent_deliveries do
  local delivery = p.recent_deliveries[i]
  local state = tostring(delivery.status or "unknown")
  local level = state == "failed" and "critical" or state == "succeeded" and "good" or "neutral"
  entries[#entries + 1] = { timestamp=tostring(delivery.createdAt or "—"), message=tostring(delivery.repository or "repository") .. " · " .. tostring(delivery.event or "event") .. " · " .. state, level=level }
end
if #entries == 0 then entries[1] = {timestamp="—",message="No recent delivery events",level="neutral"} end
return { kind="snapshot", widget={kind="event_log",title="GitHub delivery",entries=entries}, data={state="ready",source="fab",provenance="live"} }
