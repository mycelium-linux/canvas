-- Trader owns deterministic research and evidence. Canvas owns presentation.
local p = event.payload
if type(p) ~= "table" or (p.schema ~= "fpl.trader.backtest-visualization/v1" and p.schema ~= "fpl.trader.backtest-visualization/v2") then
  error("trader-backtest-v1 requires a supported fpl.trader.backtest-visualization contract")
end
if type(p.equity_curve) ~= "table" or #p.equity_curve == 0 then
  error("trader backtest requires a non-empty equity_curve")
end

local function fixed(value)
  if type(value) == "number" then return value end
  if type(value) ~= "table" then return nil end
  return (tonumber(value.mantissa) or 0) / (10 ^ (tonumber(value.scale) or 0))
end

local function points(input)
  local output = canvas.array()
  for i = 1, #input do
    local point = input[i]
    output[#output + 1] = {
      time = tonumber(point.sequence or point.time) or (i - 1),
      value = tonumber(point.equity or point.value),
    }
  end
  return output
end

local series = canvas.array()
series[#series + 1] = {label=tostring(p.strategy_name or "Strategy"), role="strategy", points=points(p.equity_curve)}
if type(p.benchmark_curve) == "table" and #p.benchmark_curve > 0 then
  series[#series + 1] = {label="Benchmark", role="benchmark", points=points(p.benchmark_curve)}
end
for i = 1, #(p.comparison_curves or {}) do
  local comparison = p.comparison_curves[i]
  series[#series + 1] = {label=tostring(comparison.label or "Comparison"), role="comparison", points=points(comparison.points or {})}
end

local report = ((p.summary or {}).report or {})
local strategy = report.strategy or {}
local benchmark = report.benchmark or {}
local delta = report.strategy_minus_benchmark or {}
local function percent(value) local n = fixed(value); if n == nil then return "—" end; return string.format("%+.2f%%", n * 100) end
local function ratio(value) local n = fixed(value); if n == nil then return "—" end; return string.format("%.2f", n) end
local stats = canvas.array()
stats[#stats + 1] = {label="Return", value=percent(strategy.total_return), tone=(fixed(strategy.total_return) or 0) >= 0 and "positive" or "negative"}
stats[#stats + 1] = {label="vs benchmark", value=percent(delta.total_return), tone=(fixed(delta.total_return) or 0) >= 0 and "positive" or "negative"}
stats[#stats + 1] = {label="Benchmark", value=percent(benchmark.total_return)}
stats[#stats + 1] = {label="Max drawdown", value=percent(strategy.maximum_drawdown), tone="negative"}
stats[#stats + 1] = {label="Sharpe", value=ratio(strategy.sharpe_ratio)}
stats[#stats + 1] = {label="Sortino", value=ratio(strategy.sortino_ratio)}
stats[#stats + 1] = {label="Trades", value=tostring(strategy.closed_trades or (p.summary or {}).execution_count or 0)}
stats[#stats + 1] = {label="CAGR", value=percent(strategy.annualized_return)}
stats[#stats + 1] = {label="Volatility", value=percent(strategy.annualized_volatility)}
stats[#stats + 1] = {label="Exposure", value=percent(strategy.exposure)}
stats[#stats + 1] = {label="Turnover", value=percent(strategy.turnover)}
stats[#stats + 1] = {label="Win rate", value=percent(strategy.hit_rate)}

local market = canvas.array()
for i = 1, #(p.market or {}) do
  local candle = p.market[i]
  market[#market + 1] = {time=tonumber(candle.sequence), open=tonumber(candle.open), high=tonumber(candle.high), low=tonumber(candle.low), close=tonumber(candle.close), volume=tonumber(candle.volume)}
end

local trades = canvas.array()
for i = 1, #(p.fills or p.trades or {}) do
  local trade = (p.fills or p.trades)[i]
  local label = trade.label
  if label == nil and trade.quantity ~= nil then
    label = tostring(trade.quantity)
    if trade.side == "exit" and trade.realized_pnl ~= nil then label = label .. " · P&L " .. tostring(trade.realized_pnl) end
  end
  trades[#trades + 1] = {time=tonumber(trade.sequence or trade.time), value=tonumber(trade.price or trade.equity or trade.value), side=trade.side, label=label}
end

return {
  kind = "snapshot",
  widget = {kind="quant", widget={visualization="backtest", title=tostring(p.title or "Strategy backtest"), evidence=tostring(report.evidence or "synthetic_proof"), market=market, series=series, trades=trades, stats=stats}},
  data = {
    state="ready",
    source="trader",
    window="full run",
    detail=tostring((p.summary or {}).run_id or "backtest evidence"),
    provenance=report.evidence == "market" and "live" or "sample",
  },
}
