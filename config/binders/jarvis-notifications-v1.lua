-- Visual half of the JARVIS notification contract. JARVIS remains the owner
-- of interruption policy and speech; Canvas renders the bounded delivery log.
local p = event.payload
if type(p) ~= "table" or type(p.notifications) ~= "table" then
  error("jarvis-notifications-v1 requires notifications")
end

local entries = {}
for i = math.max(1, #p.notifications - 11), #p.notifications do
  local n = p.notifications[i]
  if type(n) == "table" then
    local title = tostring(n.title or "Notification")
    local message = tostring(n.message or "")
    local delivery = n.announced == true and " ◉" or ""
    entries[#entries + 1] = {
      timestamp = tostring(n.timestamp or "--:--"),
      message = title .. (message ~= "" and (" · " .. message) or "") .. delivery,
      level = tostring(n.level or "neutral"),
    }
  end
end

return {
  kind = "snapshot",
  widget = {
    kind = "event_log",
    title = "Notifications",
    entries = entries,
    query = nil,
    paused = false,
    follow = true,
  },
  data = {
    state = #entries > 0 and "ready" or "no_data",
    source = "jarvis:notifications",
    detail = tostring(#entries) .. " retained",
    provenance = "live",
  },
}
