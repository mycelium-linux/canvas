local p = event.payload
if type(p) ~= "table" or type(p.beads) ~= "table" or type(p.beads.items) ~= "table" then
  error("alfalfa-project-work-v1 requires beads.items")
end
local entries = {}
for i = 1, #p.beads.items do
  local bead = p.beads.items[i]
  local level = "warning"
  if bead.status == "blocked" then level = "critical"
  elseif bead.status == "in_progress" then level = "good"
  elseif bead.ready == true then level = "neutral" end
  entries[#entries + 1] = { timestamp = tostring(bead.id or "?"), message = tostring(bead.title or bead.status or "open bead"), level = level }
end
if #entries == 0 then entries[1] = { timestamp = "—", message = "no open beads", level = "neutral" } end
return { kind = "snapshot", widget = { kind = "event_log", title = "Open beads", entries = entries }, data = { state = p.activity_state == "observed" and "ready" or "partial", source = "alfalfa:" .. tostring(p.workspace_id or "?"), provenance = "live" } }
