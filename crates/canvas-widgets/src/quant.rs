//! Typed, bounded trading and quantitative visualization primitives.

use crate::{Result, bounded_label, check};
use serde::{Deserialize, Serialize};

pub const MAX_CANDLES: usize = 2_048;
pub const MAX_BOOK_LEVELS: usize = 200;
pub const MAX_TRADES: usize = 1_024;
pub const MAX_BACKTEST_POINTS: usize = 4_096;
pub const MAX_BACKTEST_SERIES: usize = 8;
pub const MAX_BACKTEST_MARKERS: usize = 1_024;
pub const MAX_BACKTEST_STATS: usize = 12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "visualization", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum QuantWidget {
    Candlestick {
        symbol: String,
        interval: String,
        candles: Vec<Candle>,
        #[serde(default = "show_volume")]
        show_volume: bool,
    },
    OrderBook {
        symbol: String,
        venue: String,
        bids: Vec<BookLevel>,
        asks: Vec<BookLevel>,
    },
    TimeAndSales {
        symbol: String,
        venue: String,
        trades: Vec<TradePrint>,
    },
    /// Full-bleed MEV route instrument. Unlike dashboard primitives, this owns
    /// one coherent visual hierarchy for signal, route, and evidence.
    MevOpportunity {
        chain_id: u64,
        block_number: u64,
        block_hash: String,
        token: String,
        route: Vec<String>,
        amount_in: f64,
        amount_after_first: f64,
        amount_out: f64,
        gross_profit: f64,
        gas_cost: f64,
        builder_payment: f64,
        net_profit: f64,
        profitable: bool,
        evaluated_candidates: u32,
    },
    /// A retained research result. Drawdown is derived from the primary
    /// equity curve by the renderer so the producer cannot drift two copies.
    Backtest {
        title: String,
        evidence: BacktestEvidence,
        /// Canonical OHLCV evidence aligned to the replay sequence. Older
        /// producers may omit it, but dense research views should provide it.
        #[serde(default)]
        market: Vec<Candle>,
        #[serde(default)]
        regimes: Vec<BacktestRegime>,
        series: Vec<BacktestSeries>,
        #[serde(default)]
        trades: Vec<BacktestMarker>,
        #[serde(default)]
        stats: Vec<BacktestStat>,
    },
}

/// A validated state change produced by a connector binder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum QuantMutation {
    AppendCandle {
        candle: Candle,
    },
    ReplaceBook {
        bids: Vec<BookLevel>,
        asks: Vec<BookLevel>,
    },
    AppendTrade {
        trade: TradePrint,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Candle {
    /// Source-owned timestamp, normally Unix milliseconds.
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BookLevel {
    pub price: f64,
    pub size: f64,
    #[serde(default)]
    pub orders: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TradeSide {
    Buy,
    Sell,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TradePrint {
    pub time: i64,
    pub price: f64,
    pub size: f64,
    #[serde(default = "unknown_trade_side")]
    pub side: TradeSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum BacktestEvidence {
    SyntheticProof,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum BacktestSeriesRole {
    Strategy,
    Benchmark,
    Comparison,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BacktestSeries {
    pub label: String,
    pub role: BacktestSeriesRole,
    pub points: Vec<BacktestPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BacktestPoint {
    pub time: i64,
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BacktestRegime {
    pub time: i64,
    pub state: u8,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum BacktestMarkerSide {
    Entry,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BacktestMarker {
    pub time: i64,
    pub value: f64,
    pub side: BacktestMarkerSide,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BacktestStat {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub tone: BacktestStatTone,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum BacktestStatTone {
    #[default]
    Neutral,
    Positive,
    Negative,
}

fn unknown_trade_side() -> TradeSide {
    TradeSide::Unknown
}

fn show_volume() -> bool {
    true
}

impl QuantWidget {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Candlestick {
                symbol,
                interval,
                candles,
                ..
            } => {
                bounded_label(symbol, "quant symbol")?;
                bounded_label(interval, "quant interval")?;
                check(
                    !candles.is_empty() && candles.len() <= MAX_CANDLES,
                    "candlestick widget requires 1-2048 candles",
                )?;
                check(
                    candles.windows(2).all(|pair| pair[0].time < pair[1].time),
                    "candle timestamps must be strictly increasing",
                )?;
                check(
                    candles.iter().all(Candle::is_valid),
                    "candles require finite, consistent OHLC and non-negative volume",
                )
            }
            Self::OrderBook {
                symbol,
                venue,
                bids,
                asks,
            } => {
                bounded_label(symbol, "order book symbol")?;
                bounded_label(venue, "order book venue")?;
                validate_book(bids, asks)
            }
            Self::TimeAndSales {
                symbol,
                venue,
                trades,
            } => {
                bounded_label(symbol, "trade tape symbol")?;
                bounded_label(venue, "trade tape venue")?;
                check(
                    !trades.is_empty() && trades.len() <= MAX_TRADES,
                    "trade tape requires 1-1024 prints",
                )?;
                check(
                    trades.windows(2).all(|pair| pair[0].time <= pair[1].time),
                    "trade tape timestamps must be ordered",
                )?;
                check(
                    trades.iter().all(TradePrint::is_valid),
                    "trade tape prices and sizes must be finite and positive",
                )
            }
            Self::MevOpportunity {
                block_hash,
                token,
                route,
                amount_in,
                amount_after_first,
                amount_out,
                gross_profit,
                gas_cost,
                builder_payment,
                net_profit,
                evaluated_candidates,
                ..
            } => {
                bounded_label(token, "MEV token")?;
                bounded_label(block_hash, "MEV block hash")?;
                check(
                    route.len() == 2,
                    "MEV opportunity requires exactly two route legs",
                )?;
                for venue in route {
                    bounded_label(venue, "MEV venue")?;
                }
                check(
                    *evaluated_candidates > 0,
                    "MEV opportunity requires evaluated candidates",
                )?;
                check(
                    [
                        *amount_in,
                        *amount_after_first,
                        *amount_out,
                        *gross_profit,
                        *gas_cost,
                        *builder_payment,
                        *net_profit,
                    ]
                    .iter()
                    .all(|value| value.is_finite()),
                    "MEV opportunity values must be finite",
                )
            }
            Self::Backtest {
                title,
                evidence: _,
                market,
                regimes,
                series,
                trades,
                stats,
            } => {
                bounded_label(title, "backtest title")?;
                check(
                    market.len() <= MAX_CANDLES,
                    "backtest market evidence exceeds 2048 candles",
                )?;
                check(
                    market.windows(2).all(|pair| pair[0].time < pair[1].time),
                    "backtest candle timestamps must be strictly increasing",
                )?;
                check(
                    market.iter().all(Candle::is_valid),
                    "backtest candles require finite, consistent OHLCV",
                )?;
                check(
                    regimes.len() <= MAX_CANDLES
                        && regimes.windows(2).all(|pair| pair[0].time < pair[1].time)
                        && regimes.iter().all(|item| {
                            item.time >= 0
                                && item.state < 8
                                && item.confidence.is_finite()
                                && (0.0..=1.0).contains(&item.confidence)
                        }),
                    "backtest regimes require ordered bounded states and confidence",
                )?;
                check(
                    !series.is_empty() && series.len() <= MAX_BACKTEST_SERIES,
                    "backtest requires 1-8 series",
                )?;
                check(
                    series
                        .iter()
                        .any(|item| item.role == BacktestSeriesRole::Strategy),
                    "backtest requires a strategy series",
                )?;
                for item in series {
                    bounded_label(&item.label, "backtest series label")?;
                    check(
                        !item.points.is_empty() && item.points.len() <= MAX_BACKTEST_POINTS,
                        "backtest series requires 1-4096 points",
                    )?;
                    check(
                        item.points.iter().all(|point| {
                            point.time >= 0 && point.value.is_finite() && point.value >= 0.0
                        }),
                        "backtest points require non-negative time and value",
                    )?;
                    check(
                        item.points
                            .windows(2)
                            .all(|pair| pair[0].time < pair[1].time),
                        "backtest point timestamps must be strictly increasing",
                    )?;
                }
                check(
                    trades.len() <= MAX_BACKTEST_MARKERS
                        && trades.iter().all(|marker| {
                            marker.time >= 0 && marker.value.is_finite() && marker.value >= 0.0
                        }),
                    "invalid backtest trade marker",
                )?;
                for marker in trades {
                    if let Some(label) = &marker.label {
                        bounded_label(label, "backtest marker label")?;
                    }
                }
                check(
                    stats.len() <= MAX_BACKTEST_STATS,
                    "backtest exceeds 12 summary stats",
                )?;
                for stat in stats {
                    bounded_label(&stat.label, "backtest stat label")?;
                    bounded_label(&stat.value, "backtest stat value")?;
                }
                Ok(())
            }
        }
    }

    pub fn apply(&mut self, mutation: QuantMutation) -> Result<()> {
        match (self, mutation) {
            (Self::Candlestick { candles, .. }, QuantMutation::AppendCandle { candle }) => {
                check(candle.is_valid(), "invalid OHLCV stream candle")?;
                match candles.last() {
                    Some(last) if candle.time < last.time => {
                        return Err(crate::Error(
                            "stream candle timestamp moved backwards".into(),
                        ));
                    }
                    Some(last) if candle.time == last.time => {
                        *candles.last_mut().expect("last candle exists") = candle;
                    }
                    _ => candles.push(candle),
                }
                if candles.len() > MAX_CANDLES {
                    candles.drain(..candles.len() - MAX_CANDLES);
                }
                Ok(())
            }
            (
                Self::OrderBook {
                    bids: current_bids,
                    asks: current_asks,
                    ..
                },
                QuantMutation::ReplaceBook { bids, asks },
            ) => {
                validate_book(&bids, &asks)?;
                *current_bids = bids;
                *current_asks = asks;
                Ok(())
            }
            (Self::TimeAndSales { trades, .. }, QuantMutation::AppendTrade { trade }) => {
                check(trade.is_valid(), "invalid trade print")?;
                if trades.last().is_some_and(|last| trade.time < last.time) {
                    return Err(crate::Error("trade tape timestamp moved backwards".into()));
                }
                trades.push(trade);
                if trades.len() > MAX_TRADES {
                    trades.drain(..trades.len() - MAX_TRADES);
                }
                Ok(())
            }
            _ => Err(crate::Error(
                "quant mutation does not match widget visualization".into(),
            )),
        }
    }
}

impl Candle {
    fn is_valid(&self) -> bool {
        [self.open, self.high, self.low, self.close, self.volume]
            .into_iter()
            .all(f64::is_finite)
            && self.low <= self.open.min(self.close)
            && self.high >= self.open.max(self.close)
            && self.low <= self.high
            && self.volume >= 0.0
    }
}

impl BookLevel {
    fn is_valid(&self) -> bool {
        self.price.is_finite() && self.price > 0.0 && self.size.is_finite() && self.size >= 0.0
    }
}

impl TradePrint {
    fn is_valid(&self) -> bool {
        self.time >= 0
            && self.price.is_finite()
            && self.price > 0.0
            && self.size.is_finite()
            && self.size > 0.0
    }
}

fn validate_book(bids: &[BookLevel], asks: &[BookLevel]) -> Result<()> {
    check(
        !bids.is_empty() && !asks.is_empty(),
        "order book requires bids and asks",
    )?;
    check(
        bids.len() <= MAX_BOOK_LEVELS && asks.len() <= MAX_BOOK_LEVELS,
        "order book exceeds 200 levels per side",
    )?;
    check(
        bids.iter().chain(asks).all(BookLevel::is_valid),
        "order book levels must have positive finite prices and non-negative sizes",
    )?;
    check(
        bids.windows(2).all(|pair| pair[0].price > pair[1].price),
        "bids must be strictly descending",
    )?;
    check(
        asks.windows(2).all(|pair| pair[0].price < pair[1].price),
        "asks must be strictly ascending",
    )?;
    check(bids[0].price < asks[0].price, "order book is crossed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(time: i64) -> Candle {
        Candle {
            time,
            open: 100.0,
            high: 104.0,
            low: 98.0,
            close: 102.0,
            volume: 42.0,
        }
    }

    #[test]
    fn valid_ohlcv_is_bounded_and_ordered() {
        let widget = QuantWidget::Candlestick {
            symbol: "ES".into(),
            interval: "5m".into(),
            candles: vec![candle(1), candle(2)],
            show_volume: true,
        };
        assert!(widget.validate().is_ok());
        let mut reversed = vec![candle(2), candle(1)];
        let invalid = QuantWidget::Candlestick {
            symbol: "ES".into(),
            interval: "5m".into(),
            candles: std::mem::take(&mut reversed),
            show_volume: true,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn inconsistent_ohlc_is_rejected() {
        let mut bad = candle(1);
        bad.high = 99.0;
        let widget = QuantWidget::Candlestick {
            symbol: "ES".into(),
            interval: "5m".into(),
            candles: vec![bad],
            show_volume: true,
        };
        assert!(widget.validate().is_err());
    }

    #[test]
    fn forming_candle_coalesces_and_history_stays_bounded() {
        let mut widget = QuantWidget::Candlestick {
            symbol: "ES".into(),
            interval: "5m".into(),
            candles: (0..MAX_CANDLES as i64).map(candle).collect(),
            show_volume: true,
        };
        let mut replacement = candle(MAX_CANDLES as i64 - 1);
        replacement.close = 103.0;
        widget
            .apply(QuantMutation::AppendCandle {
                candle: replacement,
            })
            .unwrap();
        let next = candle(MAX_CANDLES as i64);
        widget
            .apply(QuantMutation::AppendCandle { candle: next })
            .unwrap();
        let QuantWidget::Candlestick { candles, .. } = widget else {
            unreachable!()
        };
        assert_eq!(candles.len(), MAX_CANDLES);
        assert_eq!(candles[candles.len() - 2].close, 103.0);
        assert_eq!(candles.last().unwrap().time, MAX_CANDLES as i64);
    }

    #[test]
    fn out_of_order_stream_candle_fails_loudly() {
        let mut widget = QuantWidget::Candlestick {
            symbol: "ES".into(),
            interval: "5m".into(),
            candles: vec![candle(2)],
            show_volume: true,
        };
        assert!(
            widget
                .apply(QuantMutation::AppendCandle { candle: candle(1) })
                .unwrap_err()
                .0
                .contains("backwards")
        );
    }

    #[test]
    fn order_book_requires_sorted_uncrossed_sides() {
        let bids = vec![
            BookLevel {
                price: 99.0,
                size: 4.0,
                orders: 2,
            },
            BookLevel {
                price: 98.5,
                size: 8.0,
                orders: 3,
            },
        ];
        let asks = vec![
            BookLevel {
                price: 99.5,
                size: 5.0,
                orders: 1,
            },
            BookLevel {
                price: 100.0,
                size: 7.0,
                orders: 4,
            },
        ];
        let valid = QuantWidget::OrderBook {
            symbol: "BTC-USD".into(),
            venue: "Coinbase".into(),
            bids: bids.clone(),
            asks: asks.clone(),
        };
        assert!(valid.validate().is_ok());
        let crossed = QuantWidget::OrderBook {
            symbol: "BTC-USD".into(),
            venue: "Coinbase".into(),
            bids: vec![BookLevel {
                price: 100.0,
                size: 1.0,
                orders: 1,
            }],
            asks,
        };
        assert!(crossed.validate().is_err());
    }

    #[test]
    fn trade_tape_is_bounded_and_rejects_time_travel() {
        let trade = |time| TradePrint {
            time,
            price: 67_000.0,
            size: 0.125,
            side: TradeSide::Buy,
        };
        let mut tape = QuantWidget::TimeAndSales {
            symbol: "BTC-USD".into(),
            venue: "Coinbase".into(),
            trades: vec![trade(2)],
        };
        assert!(
            tape.apply(QuantMutation::AppendTrade { trade: trade(1) })
                .is_err()
        );
        tape.apply(QuantMutation::AppendTrade { trade: trade(3) })
            .unwrap();
        let QuantWidget::TimeAndSales { trades, .. } = tape else {
            unreachable!()
        };
        assert_eq!(trades.len(), 2);
    }

    #[test]
    fn backtest_requires_ordered_finite_strategy_evidence() {
        let series = BacktestSeries {
            label: "SMA crossover".into(),
            role: BacktestSeriesRole::Strategy,
            points: vec![
                BacktestPoint {
                    time: 1,
                    value: 10_000.0,
                },
                BacktestPoint {
                    time: 2,
                    value: 10_125.0,
                },
            ],
        };
        let valid = QuantWidget::Backtest {
            title: "Backtest".into(),
            evidence: BacktestEvidence::SyntheticProof,
            market: vec![],
            regimes: vec![],
            series: vec![series.clone()],
            trades: vec![BacktestMarker {
                time: 2,
                value: 10_125.0,
                side: BacktestMarkerSide::Exit,
                label: None,
            }],
            stats: vec![BacktestStat {
                label: "Return".into(),
                value: "+1.25%".into(),
                tone: BacktestStatTone::Positive,
            }],
        };
        assert!(valid.validate().is_ok());
        let mut reversed = series;
        reversed.points.reverse();
        let invalid = QuantWidget::Backtest {
            market: vec![],
            regimes: vec![],
            title: "Backtest".into(),
            evidence: BacktestEvidence::Market,
            series: vec![reversed],
            trades: vec![],
            stats: vec![],
        };
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .0
                .contains("strictly increasing")
        );
    }
}
