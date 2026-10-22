local trade = event.payload
if type(trade) ~= "table" then error("market-trade-v1 requires a trade") end
local side = tostring(trade.side or "unknown"):lower()
if side ~= "buy" and side ~= "sell" then side = "unknown" end
return {
  kind = "quant",
  mutation = {operation="append_trade",trade={
    time=tonumber(trade.time or trade.ts),
    price=tonumber(trade.price),
    size=tonumber(trade.size or trade.quantity),
    side=side,
  }},
}
