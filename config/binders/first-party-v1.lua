-- Owner vocabulary stays explicit. This is a summary, never a reply/send surface.
return {render = function(p, now)
assert(p.schema == "canvas.first-party/1", "first-party owner projection required")
assert(p.owner == "ohm_bridge" or p.owner == "ohmos" or p.owner == "alfalfa" or p.owner == "edgerunner", "unknown owner")
assert(p.state == "ready" or p.state == "empty" or p.state == "partial" or p.state == "disconnected", "invalid source state")
assert(type(p.rows) == "table" and #p.rows <= 64, "bounded rows required")
assert(p.provenance == "live" or p.provenance == "sample", "source provenance required")
assert(p.owner ~= "ohmos" or p.provenance == "sample", "legacy ohmOS provenance is unverified")
local nodes = #p.rows == 0 and p.rows or {}
local function label(s)
  if #s <= 128 then return s end
  local result = ""
  for _, codepoint in utf8.codes(s) do
    local part = utf8.char(codepoint)
    if #result + #part > 125 then break end
    result = result .. part
  end
  return result .. "…"
end
for _, row in ipairs(p.rows) do
  nodes[#nodes + 1] = {
    label = label(row.title),
    detail = label(row.status .. " · " .. row.detail),
    state = "neutral",
  }
end
local state = p.state == "empty" and "no_data" or p.state
if now then
  assert(p.read_at_unix_ms <= now + 5000, "future owner read")
  if now - p.read_at_unix_ms >= 20000 and state ~= "disconnected" then state = "stale" end
end
local heading = (p.owner == "ohmos" or p.owner == "ohm_bridge") and "Conversations" or (p.owner == "alfalfa" and "Project context" or "Coding sessions")
return {
  kind = "snapshot",
  widget = {kind = "status_list", title = label(heading .. " · " .. p.scope), nodes = nodes},
  data = {state = state, source = label(p.owner .. ":" .. p.source .. ":" .. p.scope),
    observed_at_unix_ms = p.read_at_unix_ms, provenance = p.provenance,
    detail = p.state == "disconnected" and "Owner unavailable" or
      ((p.provenance == "sample" and "Sample · source provenance unverified · " or "") ..
        tostring(#nodes) .. " of " .. tostring(p.total) .. " · " .. tostring(p.omitted) .. " omitted" ..
        ((p.error_count or 0) > 0 and (" · " .. tostring(p.error_count) .. " source errors") or ""))},
}
end}
