-- Project Mycelium's derived topology into Canvas's network map contract.
-- Mycelium owns discovery/merge semantics; this adapter only selects labels
-- and folds its L3 subnet and L2 VLAN views into one visible network zone.
local p = event.payload
if type(p) ~= "table" or type(p.nodes) ~= "table" or type(p.segments) ~= "table" then
  error("mycelium-topology-v1 requires nodes and segments")
end

local function scalar(value)
  if type(value) == "table" then return value[1] or value["0"] end
  return value
end

local function subnet_parts(segment)
  if type(segment.subnet) ~= "table" then return nil, nil end
  return tostring(segment.subnet[1]), tonumber(segment.subnet[2])
end

local function site_from_id(id)
  if type(id) ~= "string" then return nil end
  local slash = id:find("/", 1, true)
  return slash and id:sub(1, slash - 1) or nil
end

local function site_from_origins(origins)
  if type(origins) ~= "table" then return nil end
  for _, origin in pairs(origins) do
    local site = tostring(origin):match("^([^/]+)/")
    if site then return site end
  end
  return nil
end

local function segment_site(id, segment)
  return scalar(segment.site) or site_from_id(tostring(segment.id or id))
    or site_from_origins(segment.origins)
end

local vlan_segments = {}
local function vlan_key(site, vlan)
  return tostring(site or "") .. "\0" .. tostring(vlan)
end

local function vlan_for_ip(ip, site)
  if type(ip) ~= "string" then return nil end
  local octet = ip:match("^%d+%.%d+%.(%d+)%.%d+$")
  local vlan = octet and tonumber(octet) or nil
  if not vlan then return nil end
  return (vlan_segments[vlan_key(site, vlan)] or vlan_segments[vlan_key(nil, vlan)]) and vlan or nil
end

local segments, nodes, links = canvas.array(), canvas.array(), canvas.array()
local visible, vlan_subnets = {}, {}
for id, segment in pairs(p.segments) do
  local vlan = tonumber(scalar(segment.vlan))
  if vlan then
    local segment_id = tostring(segment.id or id)
    visible[segment_id] = true
    vlan_segments[vlan_key(segment_site(id, segment), vlan)] = segment_id
  end
end
for id, segment in pairs(p.segments) do
  local ip, prefix = subnet_parts(segment)
  local site = segment_site(id, segment)
  local vlan = vlan_for_ip(ip, site)
  if vlan then vlan_subnets[vlan_key(site, vlan)] = {ip = ip, prefix = prefix, gateway = segment.gw} end
end
for id, segment in pairs(p.segments) do
  local vlan = tonumber(scalar(segment.vlan))
  local ip, prefix = subnet_parts(segment)
  local site = segment_site(id, segment)
  local folded = vlan and vlan_subnets[vlan_key(site, vlan)] or nil
  if vlan or not vlan_for_ip(ip, site) then
    local segment_id = tostring(segment.id or id)
    segments[#segments + 1] = {
      id = segment_id,
      label = vlan and ("VLAN " .. tostring(vlan)) or tostring(segment.domain_name or segment_id),
      kind = tostring(segment.kind or "lan"),
      site = site and tostring(site) or nil,
      vlan = vlan,
      subnet = folded and (folded.ip .. "/" .. tostring(folded.prefix))
        or (ip and (ip .. "/" .. tostring(prefix)) or nil),
      gateway = tostring((folded and folded.gateway) or segment.gw or "") ~= ""
        and tostring((folded and folded.gateway) or segment.gw) or nil,
    }
    visible[segment_id] = true
  end
end

for id, node in pairs(p.nodes) do
  local memberships, seen, first_ip = canvas.array(), {}, nil
  local sites = canvas.array()
  if type(node.sites) == "table" then
    for _, site in pairs(node.sites) do sites[#sites + 1] = tostring(site) end
  end
  local primary_site = sites[1] or site_from_origins(node.origins)
  local function add_vlan(vlan, site)
    local segment_id = vlan and (vlan_segments[vlan_key(site, vlan)]
      or vlan_segments[vlan_key(nil, vlan)]) or nil
    if segment_id and visible[segment_id] and not seen[segment_id] then
      memberships[#memberships + 1], seen[segment_id] = segment_id, true
    end
  end
  if type(node.ips) == "table" then
    for _, record in pairs(node.ips) do
      local ip = tostring(record.addr or "")
      if not first_ip and ip ~= "" then first_ip = ip end
      add_vlan(tonumber(scalar(record.vlan)) or vlan_for_ip(ip, primary_site),
        site_from_origins(record.origins) or primary_site)
    end
  end
  if type(node.ports) == "table" then
    for port, link in pairs(node.ports) do
      local vif = type(link.a) == "table" and tonumber(scalar(link.a.vif)) or nil
      add_vlan(vif or tonumber(tostring(port):match("%.(%d+)$")), primary_site)
    end
  end
  local label = first_ip or tostring(node.id or id)
  if type(node.hostnames) == "table" and node.hostnames[1] then label = tostring(node.hostnames[1]) end
  nodes[#nodes + 1] = {
    id = tostring(node.id or id), label = label,
    kind = node.device and "device" or "host",
    segments = memberships,
    sites = sites,
    detail = first_ip or (node.mac and tostring(node.mac) or nil),
    state = "good",
  }
end

local seen_links = {}
for id, node in pairs(p.nodes) do
  if type(node.ports) == "table" then
    for port, link in pairs(node.ports) do
      local peer = type(link.b) == "table" and link.b.device or nil
      local forward, reverse = tostring(id) .. "\0" .. tostring(peer), tostring(peer) .. "\0" .. tostring(id)
      if peer and p.nodes[peer] and not seen_links[forward] and not seen_links[reverse] then
        links[#links + 1] = {
          from = tostring(id), to = tostring(peer), label = tostring(port),
          kind = link.a and link.a.vif and "trunk" or "unknown",
          state = link.state == "down" and "critical" or (link.state == "up" and "good" or "neutral"),
        }
        seen_links[forward] = true
      end
    end
  end
end

return {
  kind = "snapshot",
  widget = {kind="network_topology",title=tostring(p.title or "Mycelium network"),segments=segments,nodes=nodes,links=links,conflicts=#(p.conflicts or {})},
  data = {state=#segments > 0 and "ready" or "no_data",source="mycelium/topology",provenance="live"},
}
