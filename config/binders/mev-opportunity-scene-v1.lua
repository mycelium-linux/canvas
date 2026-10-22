local p = event.payload
if type(p) ~= "table" or p.schema ~= "mev.opportunity.v1" or type(p.route) ~= "table" then
  error("mev opportunity scene requires mev.opportunity.v1")
end
local scale = p.token == "WETH" and 1e18 or 1
return {
  kind="snapshot",
  widget={kind="quant",widget={
    visualization="mev_opportunity",
    chain_id=tonumber(p.chain_id), block_number=tonumber(p.block_number), block_hash=tostring(p.block_hash),
    token=tostring(p.token), route=p.route,
    amount_in=(tonumber(p.amount_in) or 0)/scale,
    amount_after_first=tonumber(p.amount_after_first) or 0,
    amount_out=(tonumber(p.amount_out) or 0)/scale,
    gross_profit=(tonumber(p.gross_profit) or 0)/scale,
    gas_cost=(tonumber(p.gas_cost) or 0)/scale,
    builder_payment=(tonumber(p.builder_payment) or 0)/scale,
    net_profit=(tonumber(p.net_profit) or 0)/scale,
    profitable=p.profitable == true,
    evaluated_candidates=tonumber(p.evaluated_candidates) or 0,
  }},
  data={state="ready",source="unibus:mev-observer",detail="block-pinned reconstruction · no signer",provenance="live"}
}
