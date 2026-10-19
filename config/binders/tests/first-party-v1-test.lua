local render=dofile("config/binders/first-party-v1.lua").render
local p={schema="canvas.first-party/1",owner="ohmos",source="phone",scope="personal",state="ready",read_at_unix_ms=10000,total=1,omitted=0,rows={{id="1",title="A person",status="2 unread",detail="signal"}}}
p.provenance="sample"
local result=render(p)
assert(result.widget.nodes[1].detail=="2 unread · signal")
assert(result.data.observed_at_unix_ms==10000)
assert(result.data.provenance=="sample")
assert(render(p,30000).data.state=="stale")
assert(not pcall(render,p,4000))
p.rows[1].title=string.rep("é",100)
result=render(p)
assert(#result.widget.nodes[1].label<=128 and utf8.len(result.widget.nodes[1].label))
p.state="disconnected"; p.rows={}; p.total=0
assert(render(p).data.state=="disconnected")
p.state="empty"
assert(render(p).data.state=="no_data")
p.owner="invented"
assert(not pcall(render,p))
p.owner="ohmos"; p.provenance="live"
assert(not pcall(render,p))
print("First-party binder contract passed")
