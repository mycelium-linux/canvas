local p=event.payload
if type(p)~="table" or type(p.tracks)~="table" or type(p.notes)~="table" then error("synesthesia-daw-v1 requires tracks and notes") end
local tracks,notes=canvas.array(),canvas.array()
for i=1,#p.tracks do local t=p.tracks[i]; tracks[#tracks+1]={label=tostring(t.label),level=math.max(0,math.min(1,tonumber(t.level) or 0)),clipping=t.clipping==true} end
for i=1,#p.notes do local n=p.notes[i]; notes[#notes+1]={track=tostring(n.track),tick=math.floor(tonumber(n.tick) or 0),length_ticks=math.max(1,math.floor(tonumber(n.length_ticks) or 1)),note=math.max(0,math.min(127,math.floor(tonumber(n.note) or 0))),velocity=math.max(0,math.min(127,math.floor(tonumber(n.velocity) or 0)))} end
return {kind="snapshot",widget={kind="daw_deck",title=tostring(p.title or "Synesthesia studio"),playing=p.playing==true,paused=p.paused==true,recording=p.recording==true,bpm=tonumber(p.bpm),position_ticks=math.floor(tonumber(p.position_ticks) or 0),cycle_ticks=math.max(1,math.floor(tonumber(p.cycle_ticks) or 1)),tracks=tracks,notes=notes},data={state=p.degraded and "disconnected" or "ready",source="synesthesia/daw",provenance="live",detail=p.error and tostring(p.error) or nil}}
