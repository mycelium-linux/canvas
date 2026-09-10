-- Agent context binder: shows the project an agent is currently working on,
-- NOT a log of its actions.
--
-- The "ghost in the shell" vision (dock-ybc.21.4): agent actions manifest as
-- real workspace changes — files open via canvas_get_artifact, builds update
-- the lockout table, searches become visible artifacts. This binder provides
-- ambient context: which project is active, how many resources it has, what
-- the last tool call was.
--
-- Contract: canvas.agent.activity.v1
-- The OpenCode plugin sends lightweight events (project, tool, action);
-- the heavy lifting (opening files, running builds) happens through
-- canvas_get_artifact / lockout-status directly.

local payload = event.payload
if type(payload) ~= "table" then
  error("agent context event requires a payload table")
end

local project = payload.project or "unknown"
local tool = payload.tool or "-"
local action = payload.action or "idle"

return {
  kind = "snapshot",
  widget = {
    kind = "stat",
    label = "Active project",
    value = 0,  -- not used for stat; we use the header instead
    unit = nil,
    change = nil,
    state = nil,
    history = {},
  },
  data = {
    state = "ready",
    source = "OpenCode activity stream",
    detail = project,
    provenance = "agent",
    observed_at = payload.timestamp or "",
  },
}
