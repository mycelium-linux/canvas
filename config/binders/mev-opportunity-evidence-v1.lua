local p = event.payload
if type(p) ~= "table" or p.schema ~= "mev.opportunity.v1" then error("mev evidence requires mev.opportunity.v1") end
local hash = tostring(p.block_hash or "")
if #hash > 20 then hash = string.sub(hash, 1, 10) .. "…" .. string.sub(hash, -8) end
local rows = canvas.array()
rows[#rows+1] = {"Block", tostring(p.block_number), hash}
rows[#rows+1] = {"Gross", tostring(p.gross_profit), tostring(p.token)}
rows[#rows+1] = {"Gas", tostring(p.gas_cost), tostring(p.token)}
rows[#rows+1] = {"Builder", tostring(p.builder_payment), tostring(p.token)}
rows[#rows+1] = {"Net", tostring(p.net_profit), p.profitable and "VIABLE" or "REJECT"}
rows[#rows+1] = {"Search", tostring(p.evaluated_candidates), "bounded deterministic"}
return {kind="snapshot",widget={kind="table",title="BLOCK-PINNED EVIDENCE · CHAIN " .. tostring(p.chain_id),columns={"Measure","Value","Evidence"},rows=rows},data={state="ready",source="unibus:mev-observer",detail="read-only reconstruction · no signer",provenance="live"}}
