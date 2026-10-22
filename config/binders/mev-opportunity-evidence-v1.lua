local p = event.payload
if type(p) ~= "table" or p.schema ~= "mev.opportunity.v1" then error("mev evidence requires mev.opportunity.v1") end
local hash = tostring(p.block_hash or "")
if #hash > 20 then hash = string.sub(hash, 1, 10) .. "…" .. string.sub(hash, -8) end
local function amount(value)
  if p.token == "WETH" then return string.format("%.6f", (tonumber(value) or 0) / 1e18), "WETH" end
  return tostring(value), tostring(p.token) .. " base units"
end
local gross, gross_unit = amount(p.gross_profit)
local gas, gas_unit = amount(p.gas_cost)
local builder, builder_unit = amount(p.builder_payment)
local net = amount(p.net_profit)
local rows = canvas.array()
rows[#rows+1] = {"Block", tostring(p.block_number), hash}
rows[#rows+1] = {"Gross", gross, gross_unit}
rows[#rows+1] = {"Gas", gas, gas_unit}
rows[#rows+1] = {"Builder", builder, builder_unit}
rows[#rows+1] = {"Net", net, p.profitable and "VIABLE" or "REJECT"}
rows[#rows+1] = {"Search", tostring(p.evaluated_candidates), "bounded deterministic"}
return {kind="snapshot",widget={kind="table",title="BLOCK-PINNED EVIDENCE · CHAIN " .. tostring(p.chain_id),columns={"Measure","Value","Evidence"},rows=rows},data={state="ready",source="unibus:mev-observer",detail="read-only reconstruction · no signer",provenance="live"}}
