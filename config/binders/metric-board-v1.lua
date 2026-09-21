local p = event.payload
if type(p) ~= "table" or type(p.metrics) ~= "table" then error("metric-board-v1 requires metrics") end
local style = tostring(p.style or "radial")
if style ~= "radial" and style ~= "bullet" then error("unsupported metric board style") end
local metrics = canvas.array()
for _, metric in ipairs(p.metrics) do
  if type(metric) == "table" then
    metrics[#metrics + 1] = {
      label = tostring(metric.label or metric.name or ""), value = tonumber(metric.value) or 0,
      max = tonumber(metric.max) or 100, target = metric.target and tonumber(metric.target) or nil,
      forecast = metric.forecast and tonumber(metric.forecast) or nil,
      state = metric.state and tostring(metric.state) or nil,
    }
  end
end
return { kind="snapshot", widget={kind="metric_board",title=tostring(p.title or "Metrics"),style=style,metrics=metrics,unit=p.unit and tostring(p.unit) or nil},
  data={state=#metrics > 0 and (p.state or "ready") or "no_data",source=tostring(p.source or event.source or "metrics"),provenance="live"} }
