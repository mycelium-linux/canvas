local p=event.payload
if type(p)~="table" or type(p.groups)~="table" then error("distribution-v1 requires groups") end
local style=tostring(p.style or "box")
if style~="box" and style~="violin" then error("unsupported distribution style") end
local groups=canvas.array()
for _,group in ipairs(p.groups) do
  if type(group)=="table" and type(group.samples)=="table" then
    local samples=canvas.array()
    for _,value in ipairs(group.samples) do samples[#samples+1]=tonumber(value) or 0 end
    groups[#groups+1]={label=tostring(group.label or group.name or ""),samples=samples}
  end
end
return {kind="snapshot",widget={kind="distribution",title=tostring(p.title or "Distribution"),style=style,groups=groups,unit=p.unit and tostring(p.unit) or nil},data={state=#groups>0 and (p.state or "ready") or "no_data",source=tostring(p.source or event.source or "distribution"),provenance="live"}}
