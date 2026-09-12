-- Reusable headline-table renderer: turns any normalized headline feed
-- (Hacker News, Reddit, RSS — all shaped by canvas-feeds into the same rows)
-- into a Canvas `table` widget.
--
-- The producer sends the raw rows under the canvas.widget.snapshot.v1 contract;
-- everything a reader needs to *see* the data happens here, so changing how the
-- reel looks is editing this file, not rebuilding Canvas. This is the "lua"
-- half of the demo: presentation only, no I/O and no clock — age is computed
-- from each row's `age_secs` sent by the host fetcher, never read from one.

if type(event.payload) ~= "table" or type(event.payload.rows) ~= "table" then
  error("feed-headlines-v1 binder requires a payload rows array")
end

local MAX_ROWS = 12
local TITLE_WIDTH = 56

-- Seconds -> a human label readable across a room: "2m", "5h", "3d". The value
-- arrives already computed by the host, so this is pure formatting.
local function age(seconds)
  if type(seconds) ~= "number" or seconds < 0 then return "-" end
  if seconds < 60 then return "now" end
  local minutes = math.floor(seconds / 60)
  if minutes < 90 then return string.format("%dm", minutes) end
  local hours = math.floor(minutes / 60)
  if hours < 48 then return string.format("%dh", hours) end
  return string.format("%dd", math.floor(hours / 24))
end

-- First meaningful token of a URL, so HN/RSS links read as a source host rather
-- than an unpronounceable pathname. "news.ycombinator.com/x?id=1" ->
-- "ycombinator.com". A bare hostname collapses to itself.
local function host(url)
  if type(url) ~= "string" or url == "" then return nil end
  local needle = url:match("^%w+://(.+)") or url
  local host = needle:match("^([^/]+)")
  host = host:match("^www%.(.+)") or host
  return host
end

local function fmt(value, fallback)
  if value == nil or value == "" then return fallback end
  return tostring(value)
end

-- A story line combines the title and its host; long ones are clipped with an
-- ellipsis so the table keeps its one-line rhythm across the wall.
local function story(row)
  local title = fmt(row.title, "")
  local h = host(type(row.url) == "string" and row.url or nil)
  if h ~= nil and h ~= "" then
    title = string.format("%s — %s", title, h)
  end
  if #title > TITLE_WIDTH then
    title = title:sub(1, TITLE_WIDTH - 1) .. "…"
  end
  return title
end

local rows = {}
local payload = event.payload
local total = payload.total or #payload.rows
for index = 1, math.min(#payload.rows, MAX_ROWS) do
  local row = payload.rows[index]
  rows[#rows + 1] = {
    tostring(index),
    story(row),
    fmt(row.score, "-"),
    fmt(row.by, "-"),
    age(type(row.age_secs) == "number" and row.age_secs or nil),
  }
end

-- A source that failed to load still earns a row so the table does not silently
-- vanish — vanishing is the "looks done while being partial" failure mode.
if #rows == 0 then
  local detail = fmt(payload.error, "no headlines this cycle")
  rows[#rows + 1] = { "—", detail, "—", "—", "—" }
end

return {
  kind = "snapshot",
  widget = {
    kind = "table",
    title = (payload.title or "Headlines") ..
      (total > #rows and string.format(" (%d of %d)", #rows, total) or ""),
    columns = { "#", "story", "pts", "who", "age" },
    rows = rows,
  },
  data = {
    state = payload.error and "error" or "ready",
    source = "feed:" .. (payload.source or "unknown"),
    detail = payload.error,
    provenance = "live",
  },
}
