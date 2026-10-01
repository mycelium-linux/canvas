local p = event.payload
if type(p) ~= "table" or type(p.services) ~= "table" then
  error("home-systems-v1 requires payload.services")
end

local nodes = canvas.array()
for i = 1, #p.services do
  local service = p.services[i]
  nodes[#nodes + 1] = {
    label = tostring(service.label or service.unit or "service"),
    state = service.active and "good" or "warning",
    detail = tostring(service.detail or "no state")
  }
end

return {
  kind = "snapshot",
  widget = { kind = "status_list", title = tostring(p.host or "host") .. " · live systems", nodes = nodes },
  data = { state = "ready", source = "systemd", provenance = "live" }
}
