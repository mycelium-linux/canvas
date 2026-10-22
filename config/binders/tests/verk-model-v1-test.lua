local binder=dofile("config/binders/verk-model-v1.lua")
local input={schema="verk.model-observation.v1",id="cad-recovery",observed_at_unix_ms=1000,evidence_kind="tree-batch",requested=24,completed=10,successful=7,failed=3,training_admitted=false,disk_full_observed=true,state="incomplete",scope="teacher replay, not learned model quality"}
local fresh=binder.render(input,2000)
assert(fresh.widget.nodes[1].detail=="10 / 24")
assert(fresh.widget.nodes[6].state=="critical")
assert(fresh.data.state=="ready")
assert(binder.render(input,32000).data.state=="stale")
assert(not pcall(binder.render,input,-1))
input.completed=25
assert(not pcall(binder.render,input,2000))
print("Verk binder tests passed")
