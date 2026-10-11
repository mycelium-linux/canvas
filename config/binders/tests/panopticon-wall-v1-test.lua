local binder = dofile("config/binders/panopticon-wall-v1.lua")
local input = {schema="canvas.panopticon-wall.v1",observed_at_unix_ms=100000,observed_at="1970-01-01T00:01:40Z",omitted=0,sources={{domain="adsb",state="ready",visible=1,expired=2},{domain="ais",state="empty",visible=0,expired=0}},map={markers={{id="adsb:real"}},paths={}},selection={state="none"},distribution={known=1,ground=0,unknown=0,plot={}}}
local result = binder.render(input)
assert(result.map.frame.view == "geo_map")
assert(result.map.frame.map == input.map)
assert(result.map.data.observed_at == input.observed_at)
assert(result.evidence.body:find("AIS empty · responding",1,true))
assert(result.evidence.body:find("2 expired",1,true))
assert(not pcall(binder.render,{schema="wrong"}))
input.sources={{domain="adsb",state="disconnected"},{domain="ais",state="disconnected"}}
assert(binder.render(input).map.data.state == "disconnected")
input.selection={state="ready",id="adsb:real",observation={callsign="ACTUAL",latitude_deg=47.5,longitude_deg=-122.3,last_update=100,owner_updated_at="1970-01-01T00:01:40Z",owner_update_age_secs=1,ttl_secs=180,source="adsb_lol",aircraft={ground=true,ground_speed_knots=0}}}
local detail=binder.render(input).detail
assert(detail.title:find("ACTUAL",1,true))
assert(detail.body:find("Ground (owner report)",1,true))
assert(detail.body:find("0.0 knots",1,true))
assert(detail.body:find("Registration unknown",1,true))
assert(not detail.body:find("0 ft",1,true))
assert(detail.body:find("00:01:40 UTC · 1 s ago",1,true))
input.selection.observation.aircraft={barometric_altitude_ft={},ground_speed_knots={},track_deg={},registration={},type_designator={}}
detail=binder.render(input).detail
assert(detail.body:find("Altitude unknown",1,true) and detail.body:find("Ground speed unknown",1,true))
assert(binder.render(input).chart.frame.view=="plot")
input.selection={state="lost",id="adsb:gone"}
assert(binder.render(input).detail.title:find("Contact lost",1,true))
print("Panopticon binder contract passed")
