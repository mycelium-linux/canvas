//! Typed, bounded trading and quantitative visualization primitives.

use crate::{Result, bounded_label, check};
use serde::{Deserialize, Serialize};

pub const MAX_CANDLES: usize = 2_048;

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
}
