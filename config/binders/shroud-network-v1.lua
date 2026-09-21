local p = event.payload
if type(p) ~= "table" then error("shroud-network-v1 requires payload") end
if p.error then return {kind="snapshot",widget={kind="status_list",title="Network + discovery",nodes={{label="Region",state="critical",detail=tostring(p.error)}}},data={state="unavailable",source="shroud",detail=tostring(p.error)}} end
local nodes = {
  {label="NAT",state=(tonumber(p.nat_enabled) or 0) == 1 and "good" or "critical",detail=(tonumber(p.nat_enabled) or 0) == 1 and "Enabled" or "Disabled"},
  {label="DHCP + DNS",state=(tonumber(p.dnsmasq_running) or 0) == 1 and "good" or "critical",detail=(tonumber(p.dnsmasq_running) or 0) == 1 and "dnsmasq running" or "dnsmasq down"},
  {label="Service discovery",state="neutral",detail=tostring(p.service_records or 0) .. " records"},
  {label="Firewall",state="neutral",detail=tostring(p.iptables_rules or 0) .. " managed rules"},
  {label="Lifecycle",state="neutral",detail=tostring(p.starts or 0) .. " starts · " .. tostring(p.stops or 0) .. " stops"},
}
return {kind="snapshot",widget={kind="status_list",title="Network + discovery",nodes=nodes},data={state="ready",source="shroud",provenance="live"}}
