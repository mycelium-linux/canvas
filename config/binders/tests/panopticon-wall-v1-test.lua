local binder = dofile("config/binders/panopticon-wall-v1.lua")
local input = {schema="canvas.panopticon-wall.v1",observed_at_unix_ms=100000,observed_at="1970-01-01T00:01:40Z",omitted=0,sources={{domain="adsb",state="ready",visible=1,expired=2},{domain="ais",state="empty",visible=0,expired=0}},map={markers={{id="adsb:real"}},paths={}}}
local result = binder.render(input)
assert(result.map.frame.view == "geo_map")
assert(result.map.frame.map == input.map)
assert(result.map.data.observed_at == input.observed_at)
assert(result.evidence.body:find("AIS · 0 current",1,true))
assert(result.evidence.body:find("2 expired",1,true))
assert(not pcall(binder.render,{schema="wrong"}))
input.sources={{domain="adsb",state="disconnected"},{domain="ais",state="disconnected"}}
assert(binder.render(input).map.data.state == "disconnected")
print("Panopticon binder contract passed")
