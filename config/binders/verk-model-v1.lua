-- Canvas composition only: the owner sanitizes evidence, Unibus delivers it.
local M = {}
function M.render(value, now_ms)
  assert(type(value)=="table" and value.schema=="verk.model-observation.v1", "invalid Verk observation")
  assert(type(value.observed_at_unix_ms)=="number" and value.observed_at_unix_ms<=now_ms+1000, "invalid observation clock")
  assert(type(value.id)=="string" and #value.id<=80, "invalid evidence identity")
  local fresh=now_ms-value.observed_at_unix_ms<=30000
  local nodes={}
  if value.evidence_kind=="tree-batch" then
    assert(type(value.requested)=="number" and type(value.completed)=="number" and value.completed<=value.requested, "invalid batch counts")
    nodes={
      {label="Replay progress",detail=value.completed.." / "..value.requested,state="neutral"},
      {label="Successful executions",detail=tostring(value.successful),state="good"},
      {label="Failed executions",detail=tostring(value.failed),state=value.failed>0 and "warning" or "neutral"},
      {label="Training admission",detail=value.training_admitted and "Declared admitted" or "Not admitted",state="warning"},
      {label="Owner status",detail=value.state.." · not inferred active",state="neutral"}}
    if value.disk_full_observed then nodes[#nodes+1]={label="Storage blocker",detail="Owner reported disk full",state="critical"} end
  elseif value.evidence_kind=="native-cad-training" then
    for _, split in ipairs({"train","validation","holdout"}) do
      local item=value[split]
      assert(type(item)=="table" and type(item.acceptable_agreement)=="number" and item.acceptable_agreement>=0 and item.acceptable_agreement<=1, "invalid measured agreement")
      nodes[#nodes+1]={label=split,detail=string.format("%.1f%% choice agreement · %d examples",item.acceptable_agreement*100,item.examples),state="neutral"}
    end
    nodes[#nodes+1]={label="Recorded steps",detail=tostring(value.completed_steps),state="neutral"}
    nodes[#nodes+1]={label="Evidence scope",detail="Not geometric success / not admission",state="warning"}
  else error("unsupported Verk evidence") end
  local title=value.evidence_kind=="tree-batch" and "CAD · teacher replay" or "CAD · recorded checkpoint"
  return {widget={kind="status_list",title=title,nodes=nodes},data={state=fresh and "ready" or "stale",provenance="live",source="unibus:verk.models.v1",detail=value.scope,observed_at_unix_ms=value.observed_at_unix_ms}}
end
return M
