-- Presentation only. Numerical samples are a completed owner SPICE artifact.
local M = {}
function M.render(value, now_ms)
  assert(value.schema == "canvas.spice-trace.v1" and value.run_state == "complete", "incomplete owner simulation")
  assert(value.scope == "owner reference circuit, not LED-Blinker-Circuit", "unexpected source scope")
  assert(value.reference_id == "owned-rc-lowpass-v1", "not the admitted RC reference")
  assert(value.probe == "OUT" and type(value.samples) == "table" and #value.samples >= 2 and #value.samples <= 4096, "invalid probe samples")
  assert(value.stop_s > value.start_s and value.sample_period_s > 0, "invalid physical timebase")
  return {
    widget = {kind="signal_view", title="RC reference · OUT voltage · 0–3 ms · 0–5.5 V",
      frame={view="scope",range=value.range,samples=value.samples}},
    data = {state="ready",provenance="sample",source="Completed Legion/ngspice reference experiment; not LED simulation"},
    evidence = {title="RC reference · completed",body="NOT the LED blinker\n\nIN → 1 kΩ → OUT\nOUT → 159 nF → GND\n\n0 → 5 V step at 100 μs\nProbe OUT · 0–3 ms\nRange 0–5.5 V\n\n" .. tostring(value.source_points) .. " adaptive SPICE points\n1024 uniform display samples\nRise 10–90%: " .. string.format("%.1f",value.rise_time_s*1e6) .. " μs\n\nngspice " .. value.simulator_version .. " · pinned owner image\nSource " .. string.sub(value.source_sha256,1,12) .. "\nFinite result · not a live instrument"}
  }
end
return M
