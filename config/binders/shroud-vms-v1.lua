local p = event.payload
if type(p) ~= "table" or type(p.vms) ~= "table" then error("shroud-vms-v1 requires vms") end
local rows = canvas.array()
for i = 1, #p.vms do
  local vm = p.vms[i]
  local health = tostring(vm.health or vm.power or "unknown")
  local id = tostring(vm.id or "vm")
  if #id > 22 then id = string.sub(id, 1, 19) .. "…" end
  rows[#rows + 1] = {id, health, tostring(vm.ip or "—"), tostring(vm.vcpu or "?"), tostring(vm.memory_mib or "?") .. " MiB"}
end
if #rows == 0 then rows[1] = {"—", tostring(p.error or "No tracked virtual machines"), "—", "—", "—"} end
return {kind="snapshot",widget={kind="table",title="Virtual machines · " .. tostring(p.region or "unknown"),columns={"VM","health","IP","vCPU","memory"},rows=rows},data={state=p.error and "unavailable" or "ready",source="shroud",detail=p.error,provenance="live"}}
