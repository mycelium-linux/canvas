-- Presentation adapter for the benchmark-neutral canvas.run.watch.v1 contract.
-- Rust owns SSE, reconnection, validation and bounds; this file owns how a run
-- reads. Transmog and LoB can publish the same contract without copying UI code.
local p = event.payload
if type(p) ~= "table" or p.schema ~= "canvas.run.watch.v1" or type(p.events) ~= "table" then
  error("eval-watch-v1 requires a canvas.run.watch.v1 snapshot")
end

local run = type(p.run) == "table" and p.run or {}
local state = p.state == "running" and "partial" or (p.state == "failed" and "error" or "ready")
local source = tostring(run.harness or "eval") .. ":" .. tostring(run.benchmark or "run")
local data = { state = state, source = source, window = "live", provenance = "live" }

local function title()
  return tostring(run.model or run.modelId or "Evaluation") .. " · " .. tostring(run.benchmark or "run")
end

if event.stream == "eval-summary" then
  local counts = p.eventCounts or { decision = 0, build = 0, assembly = 0, verification = 0 }
  local nodes = canvas.array({
    { label = "Run", state = p.state == "complete" and "good" or "warning", detail = tostring(p.state or "unknown") },
    { label = "Model", state = "neutral", detail = tostring(run.model or run.modelId or "unknown") },
    { label = "Decisions", state = "neutral", detail = tostring(counts.decision or 0) },
    { label = "Build / assembly", state = "neutral", detail = tostring((counts.build or 0) + (counts.assembly or 0)) },
    { label = "Artifacts", state = "neutral", detail = tostring(p.artifactCount or 0) },
  })
  if (p.omittedEvents or 0) > 0 then
    nodes[#nodes + 1] = { label = "Earlier events", state = "neutral", detail = tostring(p.omittedEvents) .. " retained by eval" }
  end
  return { kind = "snapshot", widget = { kind = "status_list", title = title(), nodes = nodes }, data = data }
end

if event.stream == "eval-decisions" then
  local nodes, edges, previous = canvas.array(), canvas.array(), nil
  for _, item in ipairs(p.events) do
    if item.kind == "decision" then
      local id = tostring(item.id or ("decision-" .. tostring(#nodes + 1)))
      nodes[#nodes + 1] = { id = id, label = tostring(item.title or "Decision"), group = "decision", heat = item.confidence or 0.5, state = "good" }
      if previous then edges[#edges + 1] = { from = previous, to = id, heat = 0.55 } end
      previous = id
    end
  end
  return { kind = "snapshot", widget = { kind = "dag_graph", title = "Decision trace · " .. tostring(#nodes), nodes = nodes, edges = edges }, data = data }
end

if event.stream == "eval-events" then
  local entries = canvas.array()
  for _, item in ipairs(p.events) do
    if item.kind == "build" or item.kind == "assembly" then
      entries[#entries + 1] = { timestamp = tostring(item.kind), message = tostring(item.title or "Step") .. (item.detail and (" · " .. tostring(item.detail)) or ""), level = "good" }
    end
  end
  return { kind = "snapshot", widget = { kind = "event_log", title = "Run timeline · " .. tostring(#entries) .. " steps", entries = entries, follow = true }, data = data }
end

if event.stream == "eval-verification" then
  local nodes = canvas.array()
  for _, item in ipairs(p.events) do
    if item.kind == "verification" then
      local verdict = tostring(item.verdict or "unknown")
      local level = verdict == "pass" and "good" or (verdict == "fail" and "critical" or "warning")
      nodes[#nodes + 1] = { label = tostring(item.title or "Check"), state = level, detail = verdict }
    end
  end
  if #nodes == 0 then nodes[1] = { label = "Verifier", state = p.state == "running" and "warning" or "neutral", detail = p.state == "running" and "waiting" or "no checks" } end
  return { kind = "snapshot", widget = { kind = "status_list", title = "Verification · " .. tostring(#nodes), nodes = nodes }, data = data }
end

error("unsupported eval run stream: " .. tostring(event.stream))
