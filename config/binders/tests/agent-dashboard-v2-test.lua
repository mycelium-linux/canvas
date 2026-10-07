local function fresh() return assert(loadfile(arg[1]))() end
local p=fresh()
assert(p.render(1000).data.state=='loading')
local function snapshot(seq,stamp,sessions)
 return {schema_version=2,mode='snapshot',host_id='neo',provider='codex',instance_id='1',sequence=seq,observed_at_unix_ms=stamp,sessions=sessions}
end
local waiting={id='a',state='unknown',project='Canvas',task='Agent panel',attention={reason='input',since_unix_ms=1000,timing='first_observed'}}
p.receive(snapshot(1,1000,{waiting,{id='b',state='active'},{id='c',state='idle'}}),1000)
local r=p.render(2000)
assert(r.visual.view=='attention' and r.visual.panel.fresh and r.visual.panel.input==1 and r.visual.panel.active==1)
assert(r.deck.view=='task_deck' and r.deck.panel.fresh and #r.deck.panel.items==3)
assert(r.deck.panel.items[1].project=='Canvas' and r.deck.panel.items[1].state=='input')
assert(r.widget.nodes[1].detail=='1' and r.widget.nodes[3].detail=='1' and r.widget.nodes[4].detail=='1')
assert(r.details.widget.nodes[1].label=='Canvas' and r.details.widget.nodes[1].detail:find('seen waiting 1s'))
p.receive(snapshot(0,2000,{}),2000)
assert(p.render(2000).widget.nodes[1].detail=='1')
local stale=p.render(32000)
assert(not stale.visual.panel.fresh and stale.visual.panel.input==0)
assert(not stale.deck.panel.fresh and stale.deck.panel.items[1].state=='unknown')
assert(stale.data.state=='stale' and stale.widget.nodes[1].detail=='0' and stale.widget.nodes[5].detail=='3')
assert(not stale.details.widget.nodes[1].detail:find('seen waiting'))
local ended=snapshot(2,3000,{{id='a',state='ended'}});ended.mode='event'
p.receive(ended,3000)
assert(p.render(3000).widget.nodes[1].detail=='0' and p.render(3000).widget.nodes[3].detail=='1')
local dup=snapshot(3,4000,{{id='b',state='active'},{id='b',state='active'}})
assert(not pcall(p.receive,dup,4000))
waiting.attention.since_unix_ms=9000
assert(not pcall(p.receive,snapshot(3,4000,{waiting}),4000))
local empty=fresh();empty.receive(snapshot(1,1000,{}),1000)
assert(empty.render(2000).details.widget.nodes[1].detail=='No loaded sessions')
assert(empty.render(32000).data.state=='stale')
print('agent attention: explicit waiting/details/stale/replay/event/empty/validation passed')
