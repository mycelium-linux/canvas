-- Pure composition: transport and observation freshness are Rust-owned.
local M = {}
function M.render(p)
  assert(p.schema == "canvas.panopticon-wall.v1", "Panopticon wall projection required")
  assert(type(p.observed_at_unix_ms) == "number", "owner read timestamp required")
  assert(type(p.observed_at) == "string", "owner read ISO time required")
  local lines = {}
  local connected = false
  for _, source in ipairs(p.sources) do
    assert(source.domain == "adsb" or source.domain == "ais", "unknown layer")
    if source.state == "disconnected" then
      lines[#lines+1] = string.upper(source.domain) .. " · disconnected"
    else
      connected = true
      lines[#lines+1] = source.state == "empty" and (string.upper(source.domain) .. " empty · responding") or (string.upper(source.domain) .. " " .. tostring(source.visible) .. " current · " .. tostring(source.expired) .. " expired")
    end
  end
  if p.omitted > 0 then lines[#lines+1] = tostring(p.omitted) .. " omitted by renderer budget" end
  local data = {state=connected and "ready" or "disconnected",source="Panopticon owner public-near projection",observed_at=p.observed_at,provenance="live"}
  assert(type(p.selection) == "table" and type(p.distribution) == "table", "selection and derived distribution required")
  local title, body = "Select an aircraft", "Use the map direction keys to choose a contact.\n\nPhoto unavailable: this owner feed supplies no aircraft images."
  if p.selection.state == "lost" then
    title = "Contact lost · " .. p.selection.id
    body = "This contact is absent or expired in the current owner result.\nSelection was not silently replaced."
  elseif p.selection.state == "ready" then
    local o = p.selection.observation
    title = o.callsign .. " · " .. p.selection.id
    if type(o.aircraft) == "table" then
      local a = o.aircraft
      local altitude = a.ground == true and "Ground (owner report)" or (type(a.barometric_altitude_ft) == "number" and string.format("%.0f ft barometric",a.barometric_altitude_ft) or "Altitude unknown")
      local speed = type(a.ground_speed_knots) == "number" and string.format("%.1f knots ground speed",a.ground_speed_knots) or "Ground speed unknown"
      local track = type(a.track_deg) == "number" and string.format("%.0f° ground track",a.track_deg) or "Ground track unknown"
      body = (type(a.registration) == "string" and a.registration or "Registration unknown") .. " · " .. (type(a.type_designator) == "string" and a.type_designator or "Aircraft type unknown") .. "\n\n" .. altitude .. "\n" .. speed .. "\n" .. track
    else
      body = "Vessel contact · aircraft details do not apply"
    end
    local updated = type(o.owner_updated_at) == "string" and (o.owner_updated_at:match("T(%d%d:%d%d:%d%d)") or o.owner_updated_at) or tostring(o.last_update)
    body = body .. string.format("\n\n%.4f°, %.4f°\nOwner update %s UTC · %s s ago\nTTL %s s · Source %s\n\nPhoto unavailable: no owner image supplied",o.latitude_deg,o.longitude_deg,updated,tostring(o.owner_update_age_secs or "unknown"),tostring(o.ttl_secs),type(o.source) == "string" and o.source or "unknown")
  end
  return {
    map={schema_version=2,id="panopticon-map",kind="signal_view",title="Puget Sound · live owner observations",chrome="frameless",refresh_ms=10000,frame={view="geo_map",map=p.map},data=data},
    evidence={schema_version=2,id="panopticon-source",kind="rich_card",title="Sources · TTL checked",chrome="frameless",format="markdown",body=table.concat(lines,"\n") .. "\nRead " .. (p.observed_at:match("T(%d%d:%d%d:%d%d)") or p.observed_at) .. " UTC\n" .. string.format("Altitude: %d measured / %d ground / %d unknown",p.distribution.known,p.distribution.ground,p.distribution.unknown),data=data},
    detail={schema_version=2,id="panopticon-detail",kind="rich_card",title=title,chrome="frameless",format="markdown",body=body,data=data},
    chart={schema_version=2,id="panopticon-chart",kind="signal_view",title=string.format("Altitude distribution · %d measured / %d ground / %d unknown",p.distribution.known,p.distribution.ground,p.distribution.unknown),chrome="frameless",refresh_ms=10000,frame={view="plot",plot=p.distribution.plot},data=data}
  }
end
return M
