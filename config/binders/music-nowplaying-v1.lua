-- Music now-playing binder: constructs one retained media card when playback
-- state arrives. Rust owns interpolation and rendering between snapshots; this
-- script is never part of the frame loop.
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
-- Album art is resolved by the host from the bounded live-image source. The
-- source surface can remain unplaced: it is an input, not another visible card.

local p = event.payload
if type(p) ~= "table" or type(p.track) ~= "string" then
  error("music now-playing event requires payload.track as a string")
end

local elapsed = type(p.elapsed_sec) == "number" and p.elapsed_sec or 0.0
local duration = type(p.duration_sec) == "number" and p.duration_sec or 0.0
local observed_at = type(event.emitted_at_ms) == "number" and event.emitted_at_ms or 0
local title = p.track ~= "" and p.track or "Unknown track"
local artist = type(p.artist) == "string" and p.artist ~= "" and p.artist or "Unknown artist"
local album = type(p.album) == "string" and p.album ~= "" and p.album or nil
local source = type(p.source) == "string" and p.source ~= "" and p.source or "Media"

return {
  kind = "snapshot",
  widget = {
    kind = "media_card",
    title = title,
    artist = artist,
    album = album,
    source = source,
    artwork_source = "now-playing-art",
    elapsed_seconds = elapsed,
    duration_seconds = duration,
    observed_at_ms = observed_at,
    playing = p.is_playing ~= false,
  },
  data = {
    state = "ready",
    source = source .. " via canvas.music.nowplaying.v1",
    detail = artist .. (album and " • " .. album or ""),
    provenance = "live",
  },
}
