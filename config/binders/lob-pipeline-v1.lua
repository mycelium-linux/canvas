local p = event.payload
if type(p) ~= "table" or type(p.stages) ~= "table" then
  error("lob-pipeline-v1 requires a stages array")
end

local passed, warnings, errors = 0, 0, 0
for _, stage in ipairs(p.stages) do
  if stage.passed then passed = passed + 1 end
  for _, finding in ipairs(stage.findings or {}) do
    if finding.severity == "warning" then warnings = warnings + 1 end
    if finding.severity == "error" then errors = errors + 1 end
  end
end
local total = #p.stages
local state = errors > 0 and "error" or (warnings > 0 and "partial" or "ready")
local data = { state=state, source="lob.pipeline.status.v1", window="latest run", provenance="live" }

if event.stream == "lob-fan" then
  local nodes, edges = canvas.array(), canvas.array()
  local previous = nil
  for index, stage in ipairs(p.stages) do
    local id = "stage-" .. tostring(index)
    nodes[#nodes + 1] = { id=id, label=tostring(stage.stage), group="pipeline", heat=stage.passed and 0.8 or 1.0,
      state=stage.passed and "good" or "critical" }
    if previous then edges[#edges + 1] = { from=previous, to=id, heat=0.65 } end
    previous = id
  end
  return { kind="snapshot", widget={kind="dag_graph",title="Circuit decisions · " .. tostring(p.circuit),nodes=nodes,edges=edges}, data=data }
end

if event.stream == "lob-brief" then
  local bom = p.bom or {}
  local artifacts = p.artifacts or {}
  return { kind="snapshot", widget={kind="event_log",title="Build brief · " .. tostring(p.circuit),entries=canvas.array({
    {timestamp="circuit",message=tostring(p.source or p.circuit),level="neutral"},
    {timestamp="BOM",message=tostring(bom.lines or 0) .. " lines · " .. tostring(bom.components or 0) .. " components",level="good"},
    {timestamp="artifact",message=tostring(artifacts.bom_csv or "BOM not emitted"),level=artifacts.bom_csv and "good" or "warning"},
  })}, data=data }
end

if event.stream == "lob-pipeline" then
  local entries = canvas.array()
  for _, stage in ipairs(p.stages) do
    local message = stage.findings and stage.findings[1] and stage.findings[1].message or "complete"
    entries[#entries + 1] = {timestamp=tostring(stage.stage),message=tostring(message),level=stage.passed and "good" or "critical"}
  end
  return { kind="snapshot", widget={kind="event_log",title="Pipeline · " .. tostring(passed) .. "/" .. tostring(total),entries=entries}, data=data }
end

if event.stream == "lob-confidence" then
  local completion = total == 0 and 0 or (passed * 100 / total)
  local clean = total == 0 and 0 or math.max(0, 100 - ((warnings + errors) * 100 / total))
  return { kind="snapshot", widget={kind="ranked_bar",title=p.passed and "Pipeline passed" or "Pipeline needs attention",unit="%",bars=canvas.array({
    {label="stages passed",value=completion,state=p.passed and "good" or "warning"},
    {label="finding confidence",value=clean,state=errors == 0 and "good" or "critical"},
  })}, data=data }
end

error("unsupported Legion of BOM presentation stream: " .. tostring(event.stream))
