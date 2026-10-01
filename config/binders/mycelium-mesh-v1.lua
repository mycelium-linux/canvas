-- Render Mycelium as an actual node-and-edge web. Sites and VLANs are vertices,
-- never background cards. VLAN facts come only from Mycelium topology.
local p = event.payload
if type(p) ~= "table" or type(p.peers) ~= "table" then
  error("mycelium-mesh-v1 requires payload.peers")
end

local nodes, edges = canvas.array(), canvas.array()
local sites, peer_ids = {}, {}
local warnings, site_count, vlan_count = 0, 0, 0

local function health(peer)
  local age = tonumber(peer.age_seconds) or 999999
  local cpus = math.max(tonumber(peer.logical_cpus) or 1, 1)
  local load = tonumber(peer.load_1) or 0
  local total = tonumber(peer.memory_total_bytes) or 0
  local available = tonumber(peer.memory_available_bytes) or 0
  local used = total > 0 and (100 * (1 - available / total)) or 0
  local state = "good"
  if age > 120 or used >= 95 or load / cpus >= 2 then state = "critical"
  elseif age > 30 or used >= 85 or load / cpus >= 1 then state = "warning" end
  return state, age, math.max(0, math.min(1, 1 - age / 120))
end

for i = 1, #p.peers do
  local site = tostring(p.peers[i].site or "unknown")
  if not sites[site] then
    sites[site], site_count = true, site_count + 1
    nodes[#nodes + 1] = {
      id="site:" .. site, label=site, group="sites", heat=0.35, state="neutral"
    }
  end
end

local observer
for i = 1, #p.peers do
  local peer = p.peers[i]
  local id = tostring(peer.id or peer.hostname or i)
  local site = tostring(peer.site or "unknown")
  local state, _, heat = health(peer)
  if state ~= "good" then warnings = warnings + 1 end
  nodes[#nodes + 1] = {
    id=id, label=tostring(peer.hostname or "peer"), group=site, heat=heat, state=state
  }
  edges[#edges + 1] = {from="site:" .. site, to=id, heat=0.28}
  peer_ids[id] = true
  if tostring(peer.hostname) == "home-pi" then observer = id end
end

-- The peer list is home-pi's observed gossip adjacency, not a claimed full mesh.
if observer then
  for i = 1, #p.peers do
    local peer = p.peers[i]
    local id = tostring(peer.id or peer.hostname or i)
    if id ~= observer and peer_ids[id] then
      local _, _, heat = health(peer)
      edges[#edges + 1] = {from=observer, to=id, heat=heat}
    end
  end
end

if type(p.segments) == "table" then
  for i = 1, #p.segments do
    local segment = p.segments[i]
    local vlan = tonumber(segment.vlan)
    local site = tostring(segment.site or "unknown")
    if vlan then vlan_count = vlan_count + 1 end
    local id = "segment:" .. tostring(segment.id)
    nodes[#nodes + 1] = {
      id=id,
      label=vlan and ("VLAN " .. tostring(vlan) .. " · " .. tostring(segment.label))
        or tostring(segment.label),
      group=sites[site] and site or "segments", heat=0.45, state="neutral"
    }
    if sites[site] then edges[#edges + 1] = {from="site:" .. site,to=id,heat=0.4} end
  end
end

if p.degraded then
  warnings = warnings + 1
  nodes[#nodes + 1] = {
    id="observer-error",label="observation unavailable",group="errors",heat=1,state="critical"
  }
end

return {
  kind="snapshot",
  widget={kind="neural_graph",title=string.format("Mycelium web · %d peers · %d sites · %d VLANs",
    #p.peers,site_count,vlan_count),nodes=nodes,edges=edges},
  data={
    state=(p.degraded or warnings > 0 or vlan_count == 0) and "partial" or "ready",
    source="mycelium/mesh",provenance="live",
    detail=vlan_count == 0 and "VLAN inventory not discovered by Mycelium" or nil,
  },
}
