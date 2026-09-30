-- Alfalfa-specific presentation over the generic artifact-search contract.
-- Rust owns manifest validation, CID/file I/O and indexed search. This adapter
-- owns human naming and how those results become a Canvas widget.
local p = event.payload
if type(p) ~= "table" or type(p.results) ~= "table" then
  error("alfalfa-manifest-search-v1 requires results")
end

local function text(value, fallback)
  if type(value) ~= "string" or value == "" then return fallback end
  return value
end

local function kind(media_type, path)
  media_type = string.lower(text(media_type, ""))
  path = string.lower(text(path, ""))
  if string.find(media_type, "pdf", 1, true) or string.match(path, "%.pdf$") then return "PDF" end
  if string.find(media_type, "model", 1, true)
      or string.match(path, "%.stl$") or string.match(path, "%.step$")
      or string.match(path, "%.stp$") or string.match(path, "%.glb$") then return "MODEL" end
  if string.find(media_type, "image", 1, true) then return "IMAGE" end
  if string.find(media_type, "text", 1, true) or string.match(path, "%.md$") then return "TEXT" end
  return "FILE"
end

local function folder(path)
  local value = text(path, "")
  return string.match(value, "^(.*)/[^/]+$") or "Project root"
end

local function size(bytes)
  local n = tonumber(bytes) or 0
  if n >= 1024 * 1024 * 1024 then return string.format("%.1f GB", n / (1024 * 1024 * 1024)) end
  if n >= 1024 * 1024 then return string.format("%.1f MB", n / (1024 * 1024)) end
  if n >= 1024 then return string.format("%.0f KB", n / 1024) end
  return tostring(n) .. " B"
end

local rows = {}
for i = 1, math.min(#p.results, 50) do
  local entry = p.results[i]
  if type(entry) == "table" then
    rows[#rows + 1] = {
      text(entry.display_name, text(entry.name, "Unnamed artifact")),
      kind(entry.media_type, entry.path),
      folder(entry.path),
      size(entry.bytes),
    }
  end
end

local query = text(p.query, "All files")
local revision = text(p.revision, "unknown")
local matched = tonumber(p.matched) or #rows
local more = math.max(0, matched - #rows)

return {
  kind = "snapshot",
  widget = {
    kind = "table",
    title = query,
    columns = { "Name", "Kind", "Folder", "Size" },
    rows = rows,
  },
  data = {
    state = #rows > 0 and "ready" or "no_data",
    source = "alfalfa:" .. text(p.project_id, "unknown"),
    window = "revision " .. revision,
    detail = tostring(matched) .. " matches" .. (more > 0 and (" · " .. tostring(more) .. " more") or ""),
    provenance = "live",
  },
}
