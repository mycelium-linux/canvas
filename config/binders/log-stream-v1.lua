-- Bounded log snapshots. Producers own collection and cursoring; Canvas owns
-- filtering, order, viewport, severity styling, and retained presentation.
local p = event.payload
if type(p) ~= "table" or type(p.entries) ~= "table" then
  error("log-stream-v1 requires entries")
end

local entries = canvas.array()
for _, entry in ipairs(p.entries) do
  if type(entry) == "table" then
    local raw = tostring(entry.level or entry.severity or "neutral"):lower()
    local level = (raw == "error" or raw == "critical" or raw == "fatal") and "critical"
      or ((raw == "warn" or raw == "warning") and "warning"
      or ((raw == "ok" or raw == "good" or raw == "info") and "good" or "neutral"))
    entries[#entries + 1] = {
      timestamp = tostring(entry.timestamp or entry.time or ""),
      message = tostring(entry.message or entry.line or ""),
      level = level,
    }
  end
end

return {
  kind = "snapshot",
  widget = {
    kind = "event_log",
    title = tostring(p.title or "Logs"),
    entries = entries,
    query = p.query and tostring(p.query) or nil,
    paused = p.paused == true,
    follow = p.follow ~= false,
  },
  data = {
    state = p.state or "ready",
    source = tostring(p.source or event.source or "logs"),
    provenance = "live",
  },
}
