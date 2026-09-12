-- Music now-playing binder: transforms MPD playback state into a Canvas widget
-- snapshot showing the current track, artist, and playback progress.
--
-- Contract: canvas.music.nowplaying.v1
-- Payload (from music-nowplaying-feed.py):
--   {
--     project: "dock",          -- which project this surface belongs to
--     track: "Pyramid Song",    -- track title
--     artist: "Radiohead",     -- artist name
--     album: "Kid A",           -- album name
--     elapsed_sec: 42,          -- seconds elapsed in track
--     duration_sec: 256,        -- total track duration in seconds
--     is_playing: true,         -- whether playback is active
--     cover_sha256: "..."       -- optional: sha256 of album art PNG
--   }
--
-- Renders a stat widget with track info and a time_series progress bar.
-- Album art is published as a separate asset surface when cover_sha256 is present.

local p = event.payload
if type(p) ~= "table" or type(p.track) ~= "string" then
  error("music now-playing event requires payload.track as a string")
end

local title = p.artist or "?"
if p.track and p.track ~= "" then
  if title ~= "?" then title = title .. " — " end
  title = title .. p.track
end

local progress = 0.0
if type(p.elapsed_sec) == "number" and type(p.duration_sec) == "number"
  and p.duration_sec > 0 then
  progress = (p.elapsed_sec / p.duration_sec) * 100.0
  if progress > 100.0 then progress = 100.0 end
end

local state = "ready"
if p.is_playing == false then state = "neutral" end

-- Format elapsed / duration as MM:SS
local function fmt_time(sec)
  if type(sec) ~= "number" then return "--:--" end
  local m = math.floor(sec / 60)
  local s = math.floor(sec % 60)
  return string.format("%d:%02d", m, s)
end

local time_label = fmt_time(p.elapsed_sec) .. " / " .. fmt_time(p.duration_sec)

return {
  kind = "snapshot",
  widget = {
    schema_version = 1,
    id = "now-playing-" .. (p.project or "dock"),
    kind = "stat",
    label = title,
    value = progress,
    unit = time_label,
    state = state,
    history = {},
  },
  data = {
    state = state,
    source = "MPD via canvas.music.nowplaying.v1",
    detail = (p.album or "") .. (p.artist and " • " or "") .. (p.artist or ""),
    provenance = "live",
  },
}
