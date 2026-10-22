-- Presentation only. Identity is acknowledged after GPU upload and emission.
local M = {}
function M.render(value, now_ms)
  assert(value.schema == "canvas.model-showcase.v1", "invalid model observation")
  assert(type(value.catalog_count) == "number" and value.catalog_count > 0, "invalid catalog")
  assert(type(value.observed_at_unix_ms) == "number" and value.observed_at_unix_ms <= now_ms + 1000, "invalid clock")
  local fresh = now_ms - value.observed_at_unix_ms <= 30000
  local current = value.current
  local title = "Verk · preparing geometry"
  local details = "Awaiting renderer acknowledgement"
  if type(current) == "table" then
    assert(value.identity_state == "renderer_acknowledged", "unacknowledged identity")
    assert(type(current.sha256) == "string" and #current.sha256 == 64, "invalid model identity")
    local name = string.match(current.step, "/([^/]+)/result/rebuilt%.step$")
    assert(name ~= nil and #name <= 150, "invalid owner model name")
    local short = string.match(name, "^(.-)%-[A-Za-z0-9]+$") or name
    title = string.sub(short, 1, 28)
    details = "ID " .. string.sub(current.sha256, 1, 12) .. "\n\nRecovered features"
    if type(current.feature_counts) == "table" then
      local keys = {}
      for key, count in pairs(current.feature_counts) do
        assert(type(key) == "string" and #key <= 80 and type(count) == "number" and count >= 0, "invalid feature count")
        keys[#keys+1] = key
      end
      table.sort(keys)
      if #keys == 0 then details = details .. "\nOwner recorded no recognized features" end
      for i, key in ipairs(keys) do
        if i <= 6 then details = details .. "\n" .. string.gsub(key, "_", " ") .. " × " .. tostring(current.feature_counts[key]) end
      end
      if #keys > 6 then details = details .. "\n+ " .. tostring(#keys-6) .. " feature types" end
    else details = details .. "\nMetadata pending / unavailable" end
    details = details .. "\n\nNative readback verified\n" .. (current.training_admitted and "Owner declared admission" or "Not training-admitted")
  end
  local body = details .. "\n" .. tostring(value.catalog_count) .. " parts · owner catalog"
  if value.failed_previews > 0 then body = body .. "\n" .. tostring(value.failed_previews) .. " failed previews (skipped)" end
  return {
    widget = {kind="rich_card",title=title,body=body,format="markdown",chrome="frameless"},
    data = {state=fresh and (type(current) == "table" and "ready" or "no_data") or "stale",provenance="live",
      source="Verk reconstruction receipts / Canvas GPU acknowledgement",observed_at_unix_ms=value.observed_at_unix_ms}
  }
end
return M
