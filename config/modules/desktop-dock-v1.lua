-- The host supplies real open windows. Dots mean open, never stream health.
local i,t=module.inputs,module.theme
local w,h=math.max(i.width,1),math.max(i.height,1)
local function x(px) return -4+8*px/w end
local function y(px) return 4-8*px/h end
local tile,gap,inset=112,8,12
local visible=math.max(0,math.min(6,math.floor((w-2*inset-100)/120),i.count))
local total=2*inset+84+(visible>0 and 12+visible*(tile+gap)-gap or 0)
local left=(w-total)/2
widget.panel({id="ground",x=x(left),y=y(0),width=8*total/w,height=8,ink="surface",opacity=1,radius=t.radius})
widget.button({id="launcher",label="⌘  Apps",appearance="quiet",action="launcher",body={origin="desktop-dock"},x=x(left+inset),y=y(10),width=8*84/w,height=8*44/h})
if visible>0 then
  widget.panel({id="separator",x=x(left+inset+92),y=y(16),width=8/w,height=8*32/h,ink="muted",opacity=0.25,radius=0})
end
for n=0,visible-1 do
  local key="item"..n
  local px=left+inset+96+n*(tile+gap)
  local focused=i[key.."-focused"]==true
  local minimized=i[key.."-minimized"]==true
  widget.button({id=key,label=i[key.."-title"],appearance="quiet",action="focus",body={surface_id=i[key.."-id"]},x=x(px),y=y(10),width=8*tile/w,height=8*44/h})
  widget.panel({id=key.."-state",x=x(px+(tile-(focused and 28 or 6))/2),y=y(58),width=8*(focused and 28 or 6)/w,height=8*3/h,ink=focused and "primary" or "muted",opacity=minimized and 0.35 or 1,radius=2})
end
