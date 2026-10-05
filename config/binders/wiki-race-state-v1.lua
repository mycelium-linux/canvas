-- Typed controller projection. No timing inference or race orchestration.
local r = event.payload and event.payload.race
if type(r) ~= "table" or r.schema_version ~= 1 then error("wiki race state requires schema_version 1") end
local function text(value, name)
  if type(value) ~= "string" or #value < 1 or #value > 240 or value:find("[%c]") then
    error("invalid wiki race " .. name)
  end
  return {type="text",value=value}
end
local function integer(value, name, minimum, maximum)
  if type(value) ~= "number" or value ~= value or value < minimum or value > maximum or value % 1 ~= 0 then
    error("invalid wiki race " .. name)
  end
  return {type="number",value=value}
end
local phases = {ready="READY",racing="RACING",holding="HOLDING",finished="FINISHED",stopped="STOPPED"}
local phase = phases[r.phase]
if not phase then error("invalid wiki race phase") end
local inputs = {
  start=text(r.start_page,"start_page"), target=text(r.target_page,"target_page"),
  raceid=text(r.race_id,"race_id"), revision=integer(r.revision,"revision",1,9007199254740991),
  clicks=integer(r.clicks,"clicks",0,1000000),
  racems=integer(r.race_elapsed_ms,"race_elapsed_ms",0,604800000),
  wallms=integer(r.wall_elapsed_ms,"wall_elapsed_ms",0,604800000),
  observed=integer(r.observed_at_ms,"observed_at_ms",1,9007199254740991),
  page=text(r.current_page or r.start_page,"current_page"),
  trace=text(r.trace or "Waiting for next controller event","trace"),
  connected={type="boolean",value=true},
  status={type="status",level=r.phase == "stopped" and "warning" or "good",label=phase},
  raceclock={type="text",value="00:00.00"}, wallclock={type="text",value="00:00.00"},
}
if r.race_elapsed_ms > r.wall_elapsed_ms then error("race elapsed exceeds wall elapsed") end
return {kind="snapshot",widget={kind="module",module="wiki-race-v1",inputs=inputs},
  data={state="ready",source="wiki-race-controller",provenance="live"}}
