-- Agent activity binder: transforms OpenCode tool-call events into a
-- live table surface showing what the agent is doing right now.
--
-- Contract: canvas.agent.activity.v1
-- Payload (from OpenCode plugin):
--   {
--     project: "dock",            -- which project the agent is working on
--     tool: "str_replace",        -- OpenCode tool name
--     action: "edit",             -- read | edit | write | delete | tool
--     file: "crates/canvas/src/main.rs",
--     duration_ms: 42,            -- optional: how long the call took
--     success: true               -- optional: did it succeed
--     timestamp: "2026-09-10T12:30:00"
--   }
--
-- Each event replaces the table with the current single action. The producer
-- can batch recent events into one payload for a rolling history view.

local ACTIVITY = {
  read = "Read",
  edit = "Edit",
  write = "Write",
  delete = "Delete",
  tool = "Tool",
}

local payload = event.payload
if type(payload) ~= "table" or type(payload.action) ~= "string" then
  error("agent activity event requires payload.action as a string")
end

local time_str = "just now"
if type(payload.timestamp) == "string" then
  time_str = payload.timestamp:sub(-8, -3)  -- HH:MM from ISO timestamp
end

local file_label = payload.file or "?"
if type(file_label) == "string" and #file_label > 32 then
  file_label = "..." .. file_label:sub(-29)
end

local status = "ok"
if payload.success == false then status = "FAIL" end

local duration = ""
if type(payload.duration_ms) == "number" then
  duration = string.format("%dms", payload.duration_ms)
end

local state = "ready"
if payload.action == "edit" or payload.action == "write" then
  state = "ok"
elseif payload.success == false then
  state = "error"
elseif payload.action == "read" or payload.action == "tool" then
  state = "loading"
end

return {
  kind = "snapshot",
  widget = {
    kind = "table",
    title = "Agent: " .. (payload.project or "unknown"),
    columns = { "Time", "Action", "File", "Tool", "Duration", "Status" },
    rows = {
      {
        time_str,
        ACTIVITY[payload.action] or payload.action,
        file_label,
        payload.tool or "",
        duration,
        status,
      },
    },
  },
  data = {
    state = state,
    source = "OpenCode activity stream",
    detail = payload.tool or "agent active",
    provenance = "agent/" .. (payload.project or "unknown"),
  },
}
