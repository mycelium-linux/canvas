-- Pure presentation adapter for canvas.social.trends.v1. Networking, filtering,
-- deduplication, and ranking remain in the credential-bearing Rust producer.
if type(event.payload) ~= "table" or type(event.payload.items) ~= "table" or type(event.payload.status) ~= "table" then
  error("x-trends-v1 requires typed items and status")
end

local function clip(value, width)
  value = tostring(value or "")
  if #value <= width then return value end
  local limit = width - 3
  local start = limit
  local byte = string.byte(value, start)
  while start > 1 and byte >= 0x80 and byte < 0xC0 do
    start = start - 1
    byte = string.byte(value, start)
  end
  local length = byte < 0x80 and 1 or (byte < 0xE0 and 2 or (byte < 0xF0 and 3 or 4))
  local finish = start + length - 1 <= limit and start + length - 1 or start - 1
  return string.sub(value, 1, finish) .. "..."
end
local function age(seconds)
  if type(seconds) ~= "number" then return "-" end
  if seconds < 60 then return "now" end
  if seconds < 3600 then return string.format("%dm", math.floor(seconds / 60)) end
  return string.format("%dh", math.floor(seconds / 3600))
end

local rows = {}
for i = 1, math.min(#event.payload.items, 12) do
  local item = event.payload.items[i]
  rows[#rows + 1] = { tostring(i), "@" .. clip(item.author, 16), clip(item.text, 72), tostring(item.engagement_score or 0), age(item.age_secs) }
end
if #rows == 0 then rows[1] = { "-", "X", clip(event.payload.status.detail, 72), "-", "-" } end
local degraded = event.payload.status.health == "degraded"
return {
  kind = "snapshot",
  widget = { kind = "table", title = (degraded and "DEGRADED | " or "LIVE | ") .. (event.payload.title or "Social / War Room"), columns = { "#", "account", "post", "eng", "age" }, rows = rows },
  data = { state = degraded and "error" or "ready", source = "social:trend-feed", detail = event.payload.status.mode .. " | " .. event.payload.status.detail, provenance = "live" },
}
