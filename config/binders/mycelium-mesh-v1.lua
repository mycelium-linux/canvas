-- Render Mycelium peer gossip and authoritative L2/L3 segments as one graph.
-- Site lanes are derived from peer identity; VLANs come only from Mycelium's
-- topology store. The adapter never guesses VLAN membership from an address.
local p = event.payload
if type(p) ~= "table" or type(p.peers) ~= "table" then
  error("mycelium-mesh-v1 requires payload.peers")
end

local segments, nodes, links = canvas.array(), canvas.array(), canvas.array()
local sites, node_ids = {}, {}
local warnings, vlan_count, site_count = 0, 0, 0

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
  return state, age, load, cpus, used
end

for i = 1, #p.peers do
  local peer = p.peers[i]
  local site = tostring(peer.site or "unknown")
  if not sites[site] then
    sites[site], site_count = true, site_count + 1
    segments[#segments + 1] = {
      id = "site:" .. site, label = site, kind = "lan", site = site,
    }
  end
end

if type(p.segments) == "table" then
  for i = 1, #p.segments do
    local segment = p.segments[i]
    local vlan = tonumber(segment.vlan)
    if vlan then vlan_count = vlan_count + 1 end
    segments[#segments + 1] = {
      id = tostring(segment.id),
      label = vlan and (tostring(segment.label) .. " · VLAN " .. tostring(vlan))
        or tostring(segment.label),
      kind = tostring(segment.kind or (vlan and "vlan" or "lan")),
      site = segment.site and tostring(segment.site) or nil,
      vlan = vlan,
      subnet = segment.subnet and tostring(segment.subnet) or nil,
      gateway = segment.gateway and tostring(segment.gateway) or nil,
    }
  end
end

local observer
for i = 1, #p.peers do
  local peer = p.peers[i]
  local id = tostring(peer.id or peer.hostname or i)
  local state, age, load, cpus, memory = health(peer)
  if state ~= "good" then warnings = warnings + 1 end
  local root = tonumber(peer.root_available_bytes)
  nodes[#nodes + 1] = {
    id = id, label = tostring(peer.hostname or "peer"), kind = "host",
    segments = canvas.array({"site:" .. tostring(peer.site or "unknown")}),
    sites = canvas.array({tostring(peer.site or "unknown")}), state = state,
    detail = string.format("%s/%s · %.1f/%d · %.0f%% mem%s · %ds",
      tostring(peer.platform or "?"), tostring(peer.architecture or "?"),
      load, cpus, memory, root and string.format(" · %.0fG", root / 1073741824) or "", age),
  }
  node_ids[id] = true
  if tostring(peer.hostname) == "home-pi" then observer = id end
end

-- This is home-pi's observed gossip adjacency, not an invented full mesh.
if observer then
  for i = 1, #p.peers do
    local peer = p.peers[i]
    local id = tostring(peer.id or peer.hostname or i)
    if id ~= observer and node_ids[id] then
      local state, age = health(peer)
      links[#links + 1] = {
        from = observer, to = id, label = tostring(age) .. "s",
        kind = "uplink", state = state,
      }
    end
  end
end

if p.degraded then
  warnings = warnings + 1
  nodes[#nodes + 1] = {
    id = "observer-error", label = "observation unavailable", kind = "host",
    segments = canvas.array(), sites = canvas.array(), state = "critical",
    detail = tostring(p.error or "unknown error"),
  }
end

local title = string.format("Mycelium · %d peers · %d sites · %d VLANs",
  #p.peers, site_count, vlan_count)
return {
  kind = "snapshot",
  widget = {kind="network_topology",title=title,segments=segments,nodes=nodes,links=links,conflicts=0},
  data = {
    state=(p.degraded or warnings > 0 or vlan_count == 0) and "partial" or "ready",
    source="mycelium/mesh", provenance="live",
    detail=vlan_count == 0 and "VLAN inventory not discovered by Mycelium" or nil,
  },
}
