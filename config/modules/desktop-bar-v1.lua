-- Presentation only. The native shell owns these four allowlisted actions.
local i,t = module.inputs,module.theme
local w,h = math.max(i.width,1),math.max(i.height,1)
local function x(px) return -4+8*px/w end
local function y(px) return 4-8*px/h end
local function label(id,value,px,ink)
  widget.label({id=id,text=value,x=x(px),y=y((h-t.small)/2),size=t.small,ink=ink or "muted"})
end
local function control(id,value,px,width,enabled)
  widget.button({id=id,label=value,appearance="quiet",action=id,body={origin="desktop-shell"},x=x(px),y=y(6),width=8*width/w,height=8*(h-12)/h,enabled=enabled})
end
widget.panel({id="ground",x=-4,y=4,width=8,height=8,ink="surface",opacity=1,radius=0})
widget.panel({id="rule",x=-4,y=y(h-1),width=8,height=8/h,ink="muted",opacity=0.12,radius=0})
local left=12
if w>=600 then
  label("identity","fungOS",left,"primary")
  control("back","‹",left+64,28,i.back)
  control("forward","›",left+98,28,i.forward)
  left=left+134
end
local clock_width=w>=360 and 112 or 0
local project_width=math.max(32,math.min(210,w-left-clock_width-12))
control("launcher",i.workspace,left,project_width,true)
left=left+project_width+12
if w>=1000 and i.page then label("page",i.page,left) left=left+180 end
if w>=850 and i.create then control("create","＋ New",left,84,true) end
-- Registered streams are not proof of live frames. Never advertise them LIVE.
if w>=750 and i.sources>0 then label("sources",tostring(i.sources).." sources",w-220) end
if clock_width>0 then label("clock",string.sub(i.clock,1,5),w-80,"text") end
