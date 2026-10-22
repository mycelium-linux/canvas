--# selene: allow(undefined_variable, incorrect_standard_library_use)
-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Portable Canvas controls. Hosts supply bounded semantic state, never callbacks.
local i=module.inputs
local parts,projects=0,0
local partSlots={}
for n=0,i.count-1 do
  local kind=i['control'..n]
  local x,y,w,h=0,.10,.24,.14
  if kind=='project' then
    x,y,w,h=-1.72,.96-projects*.33,.86,.26;projects=projects+1
  elseif kind=='part' then
    x,y,w,h=-.40+(parts%4)*.68,.96-math.floor(parts/4)*.22,.54,.20
    partSlots[#partSlots+1]={x=x,y=y};parts=parts+1
  elseif kind=='visibility' then
    local slot=partSlots[n-i['visibility-start']+1]
    assert(slot,'visibility control has no part')
    x,y,w,h=slot.x+.30,slot.y,.055,.16
  elseif kind=='projects' then x=-1.72;w=.18
  elseif kind=='previous' then x,y,w=-.94,.63,.10
  elseif kind=='next' then x,y,w=2.15,.63,.10
  elseif kind=='geometry' then x,w=-.56,.28
  elseif kind=='fea' then x,w=-.26,.28
  elseif kind=='thermal' then x,w=.04,.28
  elseif kind=='paint' then x,w=.38,.20
  elseif kind=='review' then x,w=.64,.24
  elseif kind=='undo' then x,w=1.09,.24
  elseif kind=='assemble' then x,w,h=1.46,.30,.18
  elseif kind=='explode' then x,w,h=1.86,.30,.18
  elseif kind=='feature' then x,w=-1.10,.22
  else error('unsupported bench control '..kind) end
  local mask=n<24 and i['enabled-low'] or i['enabled-high']
  local enabled=math.floor(mask/2^(n%24))%2==1
  widget.button({id='c'..n,label=kind,action='activate',body={target_id='c'..n},
    x=x-w/2,y=y+h/2,width=w,height=h,enabled=enabled,appearance='quiet'})
end
