-- Shared typed status, not vendor MQTT. The observer validates full bounds.
local p = event.payload and event.payload.printer
if type(p) ~= "table" or p.schema_version ~= 1 or p.read_only ~= true then error("invalid read-only printer status") end
local inputs = {}
local function put(id, kind, value)
  if value ~= nil then inputs[id] = {type=kind,value=value} end
end
put("serial","text",p.serial); put("link","text",p.link)
put("observed","number",p.observed_at_unix_ms); put("expires","number",p.valid_until_unix_ms)
local t = p.telemetry or {}
for _, field in ipairs({"progress_percent","remaining_minutes","layer","layers","nozzle_c","nozzle_target_c","bed_c","bed_target_c"}) do
  local s = t[field]
  local id = field:gsub("_", "-")
  if s then put(id,"number",s.value); put(id .. "time","number",s.observed_at_unix_ms) end
end
-- Text values also have field-specific freshness: do not revive an old job.
for _, field in ipairs({"state","job"}) do
  local s = t[field]
  if s and event.emitted_at_ms-s.observed_at_unix_ms <= 60000 then put(field,"text",s.value) end
end
return {kind="snapshot",widget={kind="module",module="printer-status-v1",inputs=inputs},
  data={state="ready",source=p.observer,provenance="live"}}
