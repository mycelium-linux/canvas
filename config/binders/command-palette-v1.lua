local p=event.payload
if type(p)~="table" or type(p.results)~="table" then error("command-palette-v1 requires results") end
local results=canvas.array()
for _,result in ipairs(p.results) do results[#results+1]={id=tostring(result.id or ""),label=tostring(result.label or result.name or ""),kind=tostring(result.kind or "command"),detail=result.detail and tostring(result.detail) or nil,shortcut=result.shortcut and tostring(result.shortcut) or nil} end
return {kind="snapshot",widget={kind="command_palette",query=tostring(p.query or ""),results=results,selected=math.max(0,math.floor(tonumber(p.selected) or 0))},data={state=p.state or "ready",source=tostring(p.source or event.source or "commands"),provenance="live"}}
