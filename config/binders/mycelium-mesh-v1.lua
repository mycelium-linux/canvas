-- Render Mycelium as an actual node-and-edge web. Sites and VLANs are vertices,
-- never background cards. VLAN facts come only from Mycelium topology.
local p = event.payload
if type(p) ~= "table" or type(p.peers) ~= "table" then
  error("mycelium-mesh-v1 requires payload.peers")
end

local nodes, edges = canvas.array(), canvas.array()
local sites, peer_ids = {}, {}
local warnings, site_count, vlan_count, observed_count, service_count = 0, 0, 0, 0, 0

-- Presentation-only compatibility while the remaining legacy node is migrated.
-- Mycelium remains the owner of the raw advertised site identity.
local function display_site(site)
  if site == "home" then return "wagar-house" end
  return site
end

local function ensure_site(site)
  if not sites[site] then
    site_count = site_count + 1
    sites[site] = "site:" .. tostring(site_count)
    nodes[#nodes + 1] = {id=sites[site], label=site, group="sites", heat=0.35, state="neutral"}
  end
  return sites[site]
end

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
  local site = display_site(tostring(p.peers[i].site or "unknown"))
  ensure_site(site)
end

if type(p.nodes) == "table" then
  for i = 1, #p.nodes do
    local item = p.nodes[i]
    local site = display_site(tostring(item.site or "observed"))
    ensure_site(site)
    observed_count = observed_count + 1
    local ips = type(item.ips) == "table" and table.concat(item.ips, " · ") or ""
    local label = tostring(item.label or item.id or "observed")
    if ips ~= "" then label = label .. " · " .. ips end
    local id = "observed:" .. tostring(i)
    nodes[#nodes + 1] = {id=id,label=label,group=site,heat=item.device and 0.62 or 0.22,state="neutral"}
    edges[#edges + 1] = {from=sites[site],to=id,heat=item.device and 0.48 or 0.18}
  end
end

if type(p.advertisements) == "table" then
  for i = 1, #p.advertisements do
    local item = p.advertisements[i]
    local site = display_site(tostring(item.site or "services"))
    ensure_site(site)
    service_count = service_count + 1
    local id = "service:" .. tostring(i)
    nodes[#nodes + 1] = {id=id,label=tostring(item.label or item.service_type or "service"),group=site,heat=0.3,state="good"}
    edges[#edges + 1] = {from=sites[site],to=id,heat=0.24}
  end
end

local observer
for i = 1, #p.peers do
  local peer = p.peers[i]
  local id = "peer:" .. tostring(i)
  local site = display_site(tostring(peer.site or "unknown"))
  local state, _, heat = health(peer)
  if state ~= "good" then warnings = warnings + 1 end
  nodes[#nodes + 1] = {
    id=id, label=tostring(peer.hostname or "peer"), group=site, heat=heat, state=state
  }
  edges[#edges + 1] = {from=sites[site], to=id, heat=0.28}
  peer_ids[i] = id
  if tostring(peer.hostname) == "home-pi" then observer = id end
end

-- The peer list is home-pi's observed gossip adjacency, not a claimed full mesh.
if observer then
  for i = 1, #p.peers do
    local peer = p.peers[i]
    local id = peer_ids[i]
    if id ~= observer then
      local _, _, heat = health(peer)
      edges[#edges + 1] = {from=observer, to=id, heat=heat}
    end
  end
end

if type(p.segments) == "table" then
  for i = 1, #p.segments do
    local segment = p.segments[i]
    local vlan = tonumber(segment.vlan)
    local site = display_site(tostring(segment.site or "unknown"))
    if vlan then vlan_count = vlan_count + 1 end
    local id = "segment:" .. tostring(i)
    nodes[#nodes + 1] = {
      id=id,
      label=vlan and ("VLAN " .. tostring(vlan) .. " · " .. tostring(segment.label))
        or tostring(segment.label),
      group=sites[site] and site or "segments", heat=0.45, state="neutral"
    }
    if sites[site] then edges[#edges + 1] = {from=sites[site],to=id,heat=0.4} end
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
  widget={kind="neural_graph",title=string.format("Mycelium vantage · %d peers · %d observed · %d services · %d sites · %d VLANs",
    #p.peers,observed_count,service_count,site_count,vlan_count),nodes=nodes,edges=edges},
  data={
    state=(p.degraded or warnings > 0 or vlan_count == 0) and "partial" or "ready",
    source="mycelium/mesh",provenance="live",
    detail=vlan_count == 0 and "VLAN inventory not discovered by Mycelium" or nil,
  },
}
