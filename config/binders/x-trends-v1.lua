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

local posts = canvas.array()
for i = 1, math.min(#event.payload.items, 12) do
  local item = event.payload.items[i]
  posts[#posts + 1] = {
    id = tostring(item.id or i),
    author = clip(item.author, 48),
    text = clip(item.text, 280),
    age = age(item.age_secs),
    engagement = item.engagement_score or 0,
    matches = canvas.array(item.matches),
    url = item.url,
  }
end
local degraded = event.payload.status.health == "degraded"
return {
  kind = "snapshot",
  widget = {
    kind = "social_feed",
    title = event.payload.title or "Social / War Room",
    status = event.payload.status.health,
    detail = clip(event.payload.status.detail, 128),
    posts = posts,
    rotate_seconds = 8,
  },
  data = { state = degraded and "error" or "ready", source = "social:trend-feed", detail = clip(event.payload.status.mode .. " | " .. event.payload.status.detail, 256), provenance = "live" },
}
