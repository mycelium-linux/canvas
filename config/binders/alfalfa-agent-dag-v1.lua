local p = event.payload
local agents = type(p) == "table" and p.agents or nil
if type(agents) ~= "table" then error("alfalfa-agent-dag-v1 requires agents") end
local items = type(agents.items) == "table" and agents.items or {}
local nodes, edges, known = {}, {}, {}
local function text(value, fallback)
  if type(value) == "string" and value ~= "" then return value end
  return fallback
end
for i, agent in ipairs(items) do
  if type(agent) == "table" then
    local id = text(agent.id, text(agent.session_id, "agent-" .. tostring(i)))
    local state = text(agent.state, text(agent.status, "unknown"))
    local role = text(agent.role, text(agent.kind, "agent"))
    local label = text(agent.label, role .. " · " .. id:sub(1, 8))
    nodes[#nodes + 1] = {
      id = id, label = label, group = role,
      heat = state == "working" and 1 or (state == "idle" and 0.22 or 0.08),
      state = state == "working" and "good" or (state == "unknown" and "warning" or "neutral"),
    }
    known[id] = true
  end
end
for _, agent in ipairs(items) do
  if type(agent) == "table" then
    local id = text(agent.id, agent.session_id)
    local parent = text(agent.parent_id, agent.parent_session_id)
    if id and parent and known[id] and known[parent] and id ~= parent then
      edges[#edges + 1] = { from = parent, to = id, heat = agent.state == "working" and 1 or 0.18 }
    end
  end
end
local active_count = #nodes
if active_count == 0 then
  nodes[1] = {
    id = "no-active-agents", label = "No active agents", group = "idle",
    heat = 0, state = "neutral",
  }
end
local widget = { kind = "dag_graph", title = "Agent lineage", nodes = nodes }
if #edges > 0 then widget.edges = edges end
return {
  kind = "snapshot",
  widget = widget,
  data = {
    state = active_count > 0 and "ready" or (p.activity_state == "observed" and "no_data" or "partial"),
    source = "alfalfa:" .. tostring(p.workspace_id or "?"),
    detail = active_count > 0 and nil or "No observed agent lineage",
    provenance = "live",
  },
}
