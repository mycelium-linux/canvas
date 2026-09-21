local p = event.payload
if type(p) ~= "table" or type(p.recent_runs) ~= "table" then error("fab-runs-v1 requires recent_runs") end
local entries = canvas.array()
for i = 1, #p.recent_runs do
  local run = p.recent_runs[i]
  local state = tostring(run.status or "unknown")
  local level = (state == "failed" or state == "timed_out") and "critical" or (state == "running" or state == "publishing") and "warning" or state == "succeeded" and "good" or "neutral"
  entries[#entries + 1] = { timestamp=tostring(run.updatedAt or run.createdAt or "—"), message=tostring(run.project or "?") .. " · " .. tostring(run.kind or "build") .. " · " .. state, level=level }
end
if #entries == 0 then entries[1] = {timestamp="—",message="No retained runs",level="neutral"} end
return { kind="snapshot", widget={kind="event_log",title="Build runs",entries=entries}, data={state="ready",source="fab",provenance="live"} }
