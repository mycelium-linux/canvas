local M = {}
function M.render(p)
assert(p.schema == "canvas.modeled-trace.v1", "modeled trace required")
assert(p.source_sha256 == "580061c37e6348d8e8c26eb0e8cb04e4440640d77cd2d84a119ab509dd393ca8", "not the admitted LED source")
assert(p.probe.part == "U1" and p.probe.pin == "3" and p.supply.positive == "J1.2" and p.supply.ground == "J1.1" and p.supply.volts == 5 and p.time.stop_s == 1, "unexpected LED experiment")
local id = string.sub(p.source_sha256, 1, 12)
local data = {state="ready", provenance="sample", source="Completed owner SPICE experiment " .. id}
local current = p.led_current_max_a and string.format("LED peak %.3f mA\n",p.led_current_max_a*1000) or "LED current unavailable\n"
return {
 widget={schema_version=2,id="scope",kind="signal_view",title=p.probe.part .. "." .. p.probe.pin .. " voltage · 0–1 s",frame={view="plot",plot=p.plot},chrome="frameless",data=data},
 evidence={schema_version=2,id="evidence",kind="rich_card",title="LED blinker · " .. id,format="markdown",chrome="frameless",data=data,
 body=string.format("%s.%s · 0–1 s · %.2f–%.2f V\n%.3f Hz · %d rising edges\n",p.probe.part,p.probe.pin,p.plot.y.minimum,p.plot.y.maximum,p.frequency_hz,p.rising_edges) .. current .. string.format("\n5 V: J1.2 positive / J1.1 ground\nDeclared supply assumption\n\nCompatible 555 transistor model\nNot characterized NE555D\nGeneric red LED, not characterized\n\nngspice %s · completed finite run\n%d adaptive → 1024 display points\n\nNative ↔ reconstructed connectivity\nNot original layout recreation",p.simulator_version,p.source_points)}
}
end
return M
