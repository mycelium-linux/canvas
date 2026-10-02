-- Render Mycelium as an actual node-and-edge web. Sites and VLANs are vertices,
-- never background cards. VLAN facts come only from Mycelium topology.
local p = event.payload
if type(p) ~= "table" or type(p.peers) ~= "table" then
  error("mycelium-mesh-v1 requires payload.peers")
end

local nodes, edges = canvas.array(), canvas.array()
local sites = {}
local segments = {}
local edge_pairs = {}
local peer_ssh = {}
local warnings, site_count, segment_count, vlan_count, observed_count, service_count, ssh_count, wireguard_count, overlay_count = 0, 0, 0, 0, 0, 0, 0, 0, 0

local function identity(value)
  return string.gsub(string.lower(tostring(value or "")), "[^%w]", "")
end

for i = 1, #p.peers do
  local peer = p.peers[i]
  local sessions = tonumber(peer.established_ssh_sessions) or 0
  if sessions > 0 then
    peer_ssh[identity(peer.hostname)] = sessions
    ssh_count = ssh_count + sessions
  end
end

-- Presentation-only compatibility while the remaining legacy node is migrated.
-- Mycelium remains the owner of the raw advertised site identity.
local function display_site(site)
  if site == "home" then return "wagar-house" end
  return site
end

local function clean_label(value)
  local label = tostring(value or "")
  if string.find(string.lower(label), "userdata:", 1, true) then return "" end
  return string.sub(label, 1, 128)
end

local function ensure_site(site)
  site = clean_label(site)
  if site == "" then return nil end
  if not sites[site] then
    site_count = site_count + 1
    sites[site] = "site:" .. tostring(site_count)
    nodes[#nodes + 1] = {id=sites[site], label=site, group="sites", heat=0.35, state="neutral"}
  end
  return sites[site]
end

local function add_edge(from, to, heat, activity)
  if not from or not to or from == to then return end
  local key = tostring(from) .. "\0" .. tostring(to)
  local existing = edge_pairs[key]
  if existing then
    existing.heat = math.max(existing.heat or 0, heat or 0)
    existing.activity = math.max(existing.activity or 0, activity or 0)
    return
  end
  local edge = {from=from,to=to,heat=heat or 0,activity=activity or 0}
  edge_pairs[key] = edge
  edges[#edges + 1] = edge
end

if type(p.segments) == "table" then
  for i = 1, #p.segments do
    local segment = p.segments[i]
    segment_count = segment_count + 1
    local vlan = tonumber(segment.vlan)
    local site = display_site(tostring(segment.site or "unknown"))
    local site_id = ensure_site(site)
    if vlan then vlan_count = vlan_count + 1 end
    local id = "segment:" .. tostring(i)
    segments[tostring(segment.id)] = id
    nodes[#nodes + 1] = {
      id=id,
      label=vlan and ("VLAN " .. tostring(vlan) .. " · " .. tostring(segment.label))
        or tostring(segment.label),
      detail=tostring(segment.kind or "segment") .. (segment.subnet and (" · " .. tostring(segment.subnet)) or "") .. (segment.gateway and (" · gateway " .. tostring(segment.gateway)) or ""),
      group=site, heat=0.45, state="neutral"
    }
    add_edge(site_id, id, 0.4, 0)
  end
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

if type(p.nodes) == "table" then
  for i = 1, #p.nodes do
    local item = p.nodes[i]
    local site = item.site and display_site(tostring(item.site)) or nil
    if site then ensure_site(site) end
    observed_count = observed_count + 1
    local label = clean_label(item.label or item.id or "observed")
    local facts = canvas.array()
    local ips = type(item.ips) == "table" and table.concat(item.ips, ", ") or ""
    if ips ~= "" then facts[#facts + 1] = ips end
    if tonumber(item.capacity_mbps) then facts[#facts + 1] = tostring(item.capacity_mbps) .. "M capacity" end
    if tonumber(item.ports) then facts[#facts + 1] = tostring(item.links_up or 0) .. "/" .. tostring(item.ports) .. " links up" end
    if tonumber(item.services) and item.services > 0 then facts[#facts + 1] = tostring(item.services) .. " services" end
    if tonumber(item.overlays) and item.overlays > 0 then
      facts[#facts + 1] = "mesh " .. tostring(item.overlays_online or 0) .. "/" .. tostring(item.overlays)
    end
    if tonumber(item.overlays_active) and item.overlays_active > 0 then
      overlay_count = overlay_count + item.overlays_active
      facts[#facts + 1] = tostring(item.overlays_active) .. " direct overlay views"
    end
    if type(item.overlay_endpoints) == "table" and #item.overlay_endpoints > 0 then
      facts[#facts + 1] = "WAN " .. table.concat(item.overlay_endpoints, ", ")
    end
    if type(item.overlay_relays) == "table" and #item.overlay_relays > 0 then
      facts[#facts + 1] = "DERP " .. table.concat(item.overlay_relays, ", ")
    end
    if type(item.routed_lans) == "table" and #item.routed_lans > 0 then
      facts[#facts + 1] = "routes " .. table.concat(item.routed_lans, ", ")
    end
    local id = "observed:" .. tostring(i)
    local state = tonumber(item.links_down) and item.links_down > 0 and "warning" or "neutral"
    local ssh_sessions = peer_ssh[identity(label)] or 0
    if ssh_sessions > 0 then facts[#facts + 1] = "SSH ×" .. tostring(ssh_sessions) end
    nodes[#nodes + 1] = {id=id,label=label,detail=string.sub(table.concat(facts, " · "),1,512),group=site,heat=item.device and 0.62 or 0.22,state=state}
    local memberships = type(item.segments) == "table" and item.segments or canvas.array()
    if #memberships == 0 and site then memberships[1] = site end
    for membership_index = 1, #memberships do
      local membership = tostring(memberships[membership_index])
      local segment_id = segments[membership]
      add_edge(segment_id, id, item.device and 0.48 or 0.18, ssh_sessions)
    end
    if type(item.overlay_sites) == "table" then
      for overlay_index = 1, #item.overlay_sites do
        local overlay_site = display_site(tostring(item.overlay_sites[overlay_index]))
        local overlay_site_id = ensure_site(overlay_site)
        add_edge(overlay_site_id, id, 0.82, 0)
      end
    end
  end
end

if type(p.advertisements) == "table" then
  for i = 1, #p.advertisements do
    local item = p.advertisements[i]
    local site = item.site and display_site(tostring(item.site)) or nil
    if site then ensure_site(site) end
    service_count = service_count + 1
    local id = "service:" .. tostring(i)
    local label = clean_label(item.label or item.service_type or "service")
    local endpoint = tostring(item.service_type or "service")
    if item.target then endpoint = endpoint .. " · " .. tostring(item.target) end
    if item.port then endpoint = endpoint .. ":" .. tostring(item.port) end
    nodes[#nodes + 1] = {id=id,label=label,detail=string.sub(endpoint,1,512),group=site,heat=0.3,state="good"}
    if site then add_edge(sites[site], id, 0.24, 0) end
  end
end

if type(p.wireguard) == "table" then
  for i = 1, #p.wireguard do
    local binding = p.wireguard[i]
    local site = display_site(tostring(binding.site or "unknown"))
    local site_id = ensure_site(site)
    wireguard_count = wireguard_count + 1
    local endpoint = tostring(binding.endpoint or "endpoint unobserved")
    local prefixes = type(binding.advertised_prefixes) == "table" and table.concat(binding.advertised_prefixes, ", ") or ""
    local detail = endpoint
    if prefixes ~= "" then detail = detail .. " · routes " .. prefixes end
    nodes[#nodes + 1] = {
      id="wireguard:" .. tostring(i), label="WG · " .. clean_label(binding.hostname),
      detail=string.sub(detail,1,512), group=site, heat=binding.endpoint and 0.8 or 0.35,
      state=binding.endpoint and "good" or "warning"
    }
    add_edge(site_id, "wireguard:" .. tostring(i), 0.7, 0)
  end
end

for i = 1, #p.peers do
  local peer = p.peers[i]
  local state = health(peer)
  if state ~= "good" then warnings = warnings + 1 end
end

if p.degraded then
  warnings = warnings + 1
  nodes[#nodes + 1] = {
    id="observer-error",label="observation unavailable",group="errors",heat=1,state="critical"
  }
end

return {
  kind="snapshot",
  widget={kind="neural_graph",title=string.format("Mesh · %d peers · %d nodes · %d svc · %d sites · %d LAN · %d VLAN · %d direct · %d WG · %d SSH",
    #p.peers,observed_count,service_count,site_count,segment_count,vlan_count,overlay_count,wireguard_count,ssh_count),nodes=nodes,edges=edges},
  data={
    state=(p.degraded or warnings > 0 or vlan_count == 0) and "partial" or "ready",
    source="mycelium/mesh",provenance="live",
    detail=vlan_count == 0 and "VLAN inventory not discovered by Mycelium" or nil,
  },
}
