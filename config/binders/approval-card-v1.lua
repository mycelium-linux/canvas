local p=event.payload
if type(p)~="table" or type(p.scope)~="table" then error("approval-card-v1 requires scope") end
local scope=canvas.array()
for _,item in ipairs(p.scope) do scope[#scope+1]=tostring(item) end
return {kind="snapshot",widget={kind="approval_card",request_id=tostring(p.request_id or ""),title=tostring(p.title or "Approval required"),summary=tostring(p.summary or ""),scope=scope,expires_at=tostring(p.expires_at or "unspecified"),provenance=tostring(p.provenance or event.source or "agent"),approve_action=tostring(p.approve_action or "approval.approve"),deny_action=tostring(p.deny_action or "approval.deny"),status=tostring(p.status or "pending")},data={state="ready",source=tostring(event.source or "approval"),provenance="live"}}
