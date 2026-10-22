local p = event.payload
if type(p) ~= "table" then error("shroud-summary-v1 requires payload") end
if p.error then return {kind="snapshot",widget={kind="status_list",title="Runtime",nodes={{label="Shroud",state="critical",detail=tostring(p.error)}}},data={state="unavailable",source="shroud",detail=tostring(p.error)}} end
local function gib(bytes) return string.format("%.1f GiB", (tonumber(bytes) or 0) / 1073741824) end
local memory_used = (tonumber(p.memory_total_bytes) or 0) - (tonumber(p.memory_available_bytes) or 0)
local disk_used = (tonumber(p.filesystem_size_bytes) or 0) - (tonumber(p.filesystem_free_bytes) or 0)
local nodes = {
  {label="Daemon",state=(tonumber(p.daemon_up) or 0) == 1 and "good" or "critical",detail=(tonumber(p.daemon_up) or 0) == 1 and "Serving metrics" or "Down"},
  {label="MicroVMs",state="good",detail=tostring(p.running_vms or "?") .. " running · " .. tostring(p.configured_vms or "?") .. " configured"},
  {label="Host memory",state="neutral",detail=gib(memory_used) .. " used · " .. gib(p.memory_available_bytes) .. " available"},
  {label="Runtime disk",state="neutral",detail=gib(disk_used) .. " used · " .. gib(p.filesystem_free_bytes) .. " free"},
  {label="Load",state="neutral",detail=string.format("%.2f · 1 minute", tonumber(p.load_1m) or 0)},
}
return {kind="snapshot",widget={kind="status_list",title="Runtime · " .. tostring(p.region or "unknown"),nodes=nodes},data={state="ready",source="shroud",provenance="live"}}
