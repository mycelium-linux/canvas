-- One cloth: same bounded primitives and semantic theme as every Lua widget.
local i,t=module.inputs,module.theme
widget.panel({id="ground",x=-4,y=4,width=8,height=8,ink="surface",opacity=1,radius=t.radius})
widget.label({id="heading",text=i["window-cycle"] and "Windows" or "Applications & spaces",x=-3.65,y=3.65,size=t.title,ink="text"})
widget.label({id="target",text="ON " .. (i.target or "This screen"),x=0.45,y=3.65,size=t.small,ink="muted"})
widget.panel({id="search-bg",x=-3.7,y=2.95,width=7.4,height=0.8,ink="primary",opacity=0.09})
widget.label({id="search",text=i["window-cycle"] and "Most recently used" or ((i.query or "")=="" and "Search workspaces, windows, apps…" or (i.query .. "▏")),x=-3.5,y=2.68,size=t.body,ink=(i.query or "")=="" and "muted" or "text"})
for n=0,5 do
  local key="row" .. n
  local title=i[key .. "-title"]
  if title then
    local y=1.85-n*0.69
    local selected=i[key .. "-selected"]==true
    if selected then widget.panel({id=key .. "-bg",x=-3.7,y=y+0.16,width=7.4,height=0.66,ink="primary",opacity=0.16}) end
    widget.button({id=key,label=title,appearance="quiet",action="select",body={origin="desktop-launcher"},x=-3.7,y=y+0.16,width=7.4,height=0.4,enabled=i[key .. "-available"]==true})
    widget.label({id=key.."-detail",text=i[key.."-detail"] or "",x=-3.5,y=y-0.29,size=t.micro,ink="muted"})
  end
end
if (i.count or 0)==0 then widget.label({id="empty",text=i.live and "No matching choices" or "Waiting for this target",x=-3.5,y=0.6,size=t.body,ink="muted"}) end
widget.label({id="health",text=i.health or "Loading…",x=-3.5,y=-2.58,size=t.micro,ink=i.live and "success" or "warning"})
widget.label({id="keys",text=i["window-cycle"] and "Alt-Tab · Shift reverses · release Alt to focus · Esc cancels" or "Click to open · ↑↓ Enter · Tab screen · Esc close",x=-3.5,y=-3.3,size=t.micro,ink="muted"})
