-- Read-only, themed presentation. Protocol/credentials stay outside Canvas.
local i, theme, now = module.inputs, module.theme, module.now.epoch_ms
local fresh = (i.observed or 0) > 0 and now >= (i.observed or 0)-2000 and now < (i.expires or 0)
local live = fresh and i.link == "live"
local function value(field, suffix)
  field = field:gsub("_", "-")
  local t = i[field .. "time"] or 0
  if not live or t == 0 or now-t > 60000 or now-t < -2000 or i[field] == nil then return "—" end
  return string.format("%.0f%s", i[field], suffix or "")
end
local status = fresh and string.upper(i.link or "waiting") or "AWAITING ENROLLMENT / STALE"
widget.panel({id="panel",ink="surface",opacity=1,x=-4,y=4,width=8,height=8})
widget.label({id="title",text="PRINTER · " .. (i.serial or "not enrolled"),x=-3.6,y=3.5,ink="primary",size=theme.title})
widget.badge({id="link",text=status,x=-3.6,y=2.5,width=6.8,height=0.65,ink=live and "success" or "warning"})
widget.label({id="job",text=live and (i.job or "No reported job") or "Waiting for authenticated printer reports",x=-3.6,y=1.5,ink="muted",size=theme.body})
widget.metric({id="progress",label=live and (i.state or "PRINT PROGRESS") or "PRINT PROGRESS",value=value("progress_percent","%"),x=-3.6,y=0.4,size=theme.display,value_offset=0.7})
widget.metric({id="time",label="REMAINING",value=value("remaining_minutes"," min"),x=0.3,y=0.4,size=theme.large,value_offset=0.7})
widget.metric({id="nozzle",label="NOZZLE / TARGET",value=value("nozzle_c","°") .. " / " .. value("nozzle_target_c","°"),x=-3.6,y=-1.1,size=theme.large,value_offset=0.65})
widget.metric({id="bed",label="BED / TARGET",value=value("bed_c","°") .. " / " .. value("bed_target_c","°"),x=0.3,y=-1.1,size=theme.large,value_offset=0.65})
widget.label({id="layers",text="LAYERS " .. value("layer") .. " / " .. value("layers"),x=-3.6,y=-2.5,ink="muted",size=theme.body})
widget.label({id="readonly",text="READ ONLY · no printer commands",x=-3.6,y=-3.3,ink="muted",size=theme.micro})
