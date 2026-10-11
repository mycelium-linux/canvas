-- Pure composition: transport and observation freshness are Rust-owned.
local M = {}
function M.render(p)
  assert(p.schema == "canvas.panopticon-wall.v1", "Panopticon wall projection required")
  assert(type(p.observed_at_unix_ms) == "number", "owner read timestamp required")
  local lines = {}
  local connected = false
  for _, source in ipairs(p.sources) do
    assert(source.domain == "adsb" or source.domain == "ais", "unknown layer")
    if source.state == "disconnected" then
      lines[#lines+1] = string.upper(source.domain) .. " · disconnected"
    else
      connected = true
      lines[#lines+1] = string.upper(source.domain) .. " · " .. tostring(source.visible) .. " current / " .. tostring(source.expired) .. " expired"
    end
  end
  lines[#lines+1] = tostring(p.omitted) .. " markers outside renderer budget"
  local data = {state=connected and "ready" or "disconnected",source="Panopticon owner public-near projection",observed_at=p.observed_at,provenance="live"}
  return {
    map={schema_version=2,id="panopticon-map",kind="signal_view",title="Puget Sound · live owner observations",chrome="frameless",refresh_ms=10000,frame={view="geo_map",map=p.map},data=data},
    evidence={schema_version=2,id="panopticon-source",kind="rich_card",title="Panopticon · Puget Sound / PNW",chrome="frameless",format="markdown",body=table.concat(lines,"\n") .. "\n\nObservation TTL enforced\nEmpty AIS is not a disconnected source\n" .. (p.boundary_source or "No basemap supplied") .. "\n\nADS-B / AIS public-near owner projection\nPrivate operator snapshot unavailable",data=data}
  }
end
return M
