local p = event.payload
if type(p) ~= "table" or type(p.peers) ~= "table" then
  error("mycelium-mesh-v1 requires payload.peers")
end

local nodes, warnings = canvas.array(), 0
for i = 1, #p.peers do
  local peer = p.peers[i]
  local age = tonumber(peer.age_seconds) or 999999
  local cpus = math.max(tonumber(peer.logical_cpus) or 1, 1)
  local load = tonumber(peer.load_1) or 0
  local memory_total = tonumber(peer.memory_total_bytes) or 0
  local memory_available = tonumber(peer.memory_available_bytes) or 0
  local memory_used = memory_total > 0 and (100 * (1 - memory_available / memory_total)) or 0
  local state = "good"
  if age > 120 or memory_used >= 95 or load / cpus >= 2 then state = "critical"
  elseif age > 30 or memory_used >= 85 or load / cpus >= 1 then state = "warning" end
  if state ~= "good" then warnings = warnings + 1 end
  local root = ""
  if tonumber(peer.root_available_bytes) then
    root = string.format(" · %.0fG free", tonumber(peer.root_available_bytes) / 1073741824)
  end
  nodes[#nodes + 1] = {
    label = tostring(peer.hostname or "peer"), state = state,
    detail = string.format("%s · %s/%s · load %.1f/%d · mem %.0f%%%s · %ds",
      tostring(peer.site or "unknown"), tostring(peer.platform or "?"),
      tostring(peer.architecture or "?"), load, cpus, memory_used, root, age)
  }
end

local releases = type(p.releases) == "table" and #p.releases or 0
local title = string.format("Mycelium mesh · %d peers · %d releases", #p.peers, releases)
if p.degraded then
  nodes[#nodes + 1] = {label="Mycelium observation unavailable",state="critical",detail=tostring(p.error or "unknown error")}
end
return {
  kind = "snapshot",
  widget = {kind="status_list",title=title,nodes=nodes},
  data = {state=(p.degraded or warnings > 0) and "partial" or "ready",source="mycelium/mesh",provenance="live"}
}
