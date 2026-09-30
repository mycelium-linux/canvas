#!/usr/bin/env lua
-- MPD lyrics producer for Canvas
-- Polls MPD and emits WidgetMutation snapshots for lyrics_display widget

local mpd = require "mpd"
local json = require "cjson"
local canvas = require "canvas-canvas"

-- Configuration
local config = {
    host = "localhost",
    port = 6600,
    password = nil,
    poll_interval_ms = 500,
    lyrics_dir = "/var/lib/mpd/lyrics"
}

-- LRC parser
local function parse_lrc(lrc_text)
    local lines = {}
    local time_pattern = "%[(%d%d):(%d%d)(%.?(%d%d?%d?)%)%]"
    local current_time = nil
    local current_text = ""
    
    for line in lrc_text:gmatch("[^\n]+") do
        line = line:match("^%s*(.-)%s*$")
        if not line or line == "" then
            continue
        end
        
        local m1, m2, ms = line:match(time_pattern)
        if m1 then
            -- Save previous line
            if current_time and current_text ~= "" then
                table.insert(lines, {
                    text = current_text:match("^%s*(.-)%s*$"),
                    start_ms = current_time.start,
                    end_ms = current_time.end,
                    words = {}
                })
            end
            
            -- Parse new timestamp
            local minutes = tonumber(m1) or 0
            local seconds = tonumber(m2) or 0
            ms = ms or "00"
            local ms_val = tonumber(ms) or 0
            -- Handle 2 or 3 digit milliseconds
            if #ms == 2 then
                ms_val = ms_val * 10
            end
            
            local start = minutes * 60 * 1000 + seconds * 1000 + ms_val
            current_time = { start = start, end = start + 2000 }
            current_text = line:gsub(time_pattern, ""):match("^%s*(.-)%s*$")
        elseif current_time then
            current_text = current_text .. " " .. line
        end
    end
    
    -- Don't forget the last line
    if current_time and current_text ~= "" then
        table.insert(lines, {
            text = current_text:match("^%s*(.-)%s*$"),
            start_ms = current_time.start,
            end_ms = current_time.end,
            words = {}
        })
    end
    
    return lines
end

-- Fetch lyrics from LRC file
local function fetch_lyrics(track_info)
    if not track_info.file then
        return {}
    end
    
    -- Check for .lrc file
    local lrc_file = track_info.file:gsub("%.mp3$", ".lrc")
                 :gsub("%.ogg$", ".lrc")
                 :gsub("%.flac$", ".lrc")
    
    local filepath = config.lyrics_dir and 
                     config.lyrics_dir .. "/" .. lrc_file or
                     track_info.file:gsub("%.mp3$", ".lrc")
    
    local file = io.open(filepath, "r")
    if file then
        local lrc = file:read("*all")
        file:close()
        return parse_lrc(lrc)
    end
    
    return {}
end

-- Main poll loop
local function poll()
    -- Connect to MPD
    local client = mpd.connect({
        host = config.host,
        port = config.port,
        password = config.password
    })
    
    if not client then
        canvas.error("Failed to connect to MPD")
        return
    end
    
    -- Get status
    local status = client:status()
    local playing = status.state == "play"
    local position = status.current_time or 0
    local duration = status.duration or 0
    local position_ms = math.floor(position * 1000)
    local duration_ms = math.floor(duration * 1000)
    
    -- Get current track
    local playlist = client:playlistentry(0)
    local track_info = {
        title = playlist.Title or "Unknown",
        artist = playlist.Artist or "Unknown Artist",
        album = playlist.Album,
        file = playlist.File
    }
    
    -- Fetch lyrics
    local lyrics = fetch_lyrics(track_info)
    
    -- Build widget spec
    local widget = {
        schema_version = 1,
        kind = "lyrics_display",
        id = "mpd-lyrics",
        title = track_info.title,
        artist = track_info.artist,
        album = track_info.album,
        lines = lyrics,
        position_ms = position_ms,
        duration_ms = duration_ms,
        playing = playing,
        style = "hyperspace",
        scroll_offset = 0.0,
        highlight_intensity = playing and 0.8 or 0.0,
        chrome = "frameless",
        data = {
            source = "mpd-client",
            provenance = "live",
            state = "ready"
        }
    }
    
    -- Emit mutation
    canvas.emit("canvas.widget.mutation", {
        widget_id = "mpd-lyrics",
        mutation = {
            kind = "snapshot",
            widget = widget,
            data = widget.data
        }
    })
    
    client:close()
end

-- Run loop
while true do
    poll()
    os.execute(string.format("sleep %.3f", config.poll_interval_ms / 1000))
end
