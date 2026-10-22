local p = event.payload
if type(p) ~= "table" or type(p.totals) ~= "table" then error("fab-metrics-v1 requires totals") end
local t = p.totals
local nodes = {
  {label="Projects",state="good",detail=tostring(t.projects or 0)},
  {label="Runs retained",state="neutral",detail=tostring(t.recentRuns or 0)},
  {label="Active websites",state=(tostring(t.failingWebsites or 0) ~= "0") and "critical" or "good",detail=tostring(t.activeWebsites or 0) .. " active · " .. tostring(t.failingWebsites or 0) .. " failing"},
  {label="Worker capacity",state="neutral",detail=tostring((p.worker or {}).concurrency or "?") .. " concurrent · " .. tostring((p.worker or {}).cpusPerRun or "?") .. " CPU/run"},
}
return { kind="snapshot", widget={kind="status_list",title="Fab control plane",nodes=nodes}, data={state="ready",source="fab",provenance="live"} }
