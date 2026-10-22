-- Agent attention binder: transforms attention events into a surface
-- showing what the agent is currently reading, editing, or searching.
--
-- Contract: canvas.agent.attention.v1
-- Payload:
--   {
--     project: "string",     -- which project the agent is working on
--     kind: "read",          -- read | edit | write | search
--     target: "path/file",   -- file path or search query
--     region: "line:col"     -- optional: specific location
--     at: "2026-09-12T12:00:00"  -- timestamp
--   }
--
-- The agent emits these events; Canvas routes them to the per-project
-- workspace's attention surface. The binder accumulates the last N events
-- and renders them as a table, with stale events fading to neutral.

local attention = event.payload
if type(attention) ~= "table" or type(attention.kind) ~= "string" then
  error("agent attention event requires payload.kind as a string")
end

-- Salience: writes and long reads are notable, not every file open.
local SALIENT = {
  write = "write",
  edit = "edit",
  read = "read",  -- long reads only, filtered by the producer
  search = "search",
}

local KIND_LABEL = {
  read = "Read",
  edit = "Edit",
  write = "Write",
  search = "Search",
}

local label = KIND_LABEL[attention.kind] or attention.kind
local target = attention.target or "unknown"
local region = attention.region or ""
local at = attention.at or ""

-- Emit a snapshot table with the most recent attention event.
-- The lockout-v1 binder already handles table mutations; this binder
-- replaces the table content with the latest attention signal.
return {
  kind = "snapshot",
  widget = {
    kind = "table",
    title = "Agent attention",
    columns = { "Kind", "Target", "Region", "At" },
    rows = {
      { label, target, region, at },
    },
  },
  data = {
    state = "ready",
    source = "agent",
    detail = string.format("%s on %s", label, target),
    provenance = "canvas.agent.attention.v1",
  },
}
