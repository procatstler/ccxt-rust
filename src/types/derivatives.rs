//! Derivatives Types
//!
//! Types for options and advanced derivatives trading

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Option contract information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionContract {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Currency code
    pub currency: String,

    /// Trading symbol
    pub symbol: String,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Implied volatility
    pub implied_volatility: Decimal,

    /// Open interest
    pub open_interest: Decimal,

    /// Bid price
    pub bid_price: Decimal,

    /// Ask price
    pub ask_price: Decimal,

    /// Mid price
    pub mid_price: Decimal,

    /// Mark price
    pub mark_price: Decimal,

    /// Last trade price
    pub last_price: Decimal,

    /// Underlying asset price
    pub underlying_price: Decimal,

    /// Price change
    pub change: Decimal,

    /// Price change percentage
    pub percentage: Decimal,

    /// Base currency volume
    pub base_volume: Decimal,

    /// Quote currency volume
    pub quote_volume: Decimal,
}

impl OptionContract {
    /// Create a new OptionContract
    pub fn new(currency: impl Into<String>, symbol: impl Into<String>) -> Self {
        OptionContract {
            info: Value::Null,
            currency: currency.into(),
            symbol: symbol.into(),
            timestamp: None,
            datetime: None,
            implied_volatility: Decimal::ZERO,
            open_interest: Decimal::ZERO,
            bid_price: Decimal::ZERO,
            ask_price: Decimal::ZERO,
            mid_price: Decimal::ZERO,
            mark_price: Decimal::ZERO,
            last_price: Decimal::ZERO,
            underlying_price: Decimal::ZERO,
            change: Decimal::ZERO,
            percentage: Decimal::ZERO,
            base_volume: Decimal::ZERO,
            quote_volume: Decimal::ZERO,
        }
    }
}

/// Option Greeks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Greeks {
    /// Trading symbol
    pub symbol: String,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Delta - rate of change of option price vs underlying price
    pub delta: Decimal,

    /// Gamma - rate of change of delta
    pub gamma: Decimal,

    /// Theta - time decay
    pub theta: Decimal,

    /// Vega - sensitivity to volatility
    pub vega: Decimal,

    /// Rho - sensitivity to interest rates
    pub rho: Decimal,

    /// Vanna - second order Greek
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vanna: Option<Decimal>,

    /// Volga - second order Greek
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volga: Option<Decimal>,

    /// Charm - rate of change of delta over time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charm: Option<Decimal>,

    /// Bid size
    pub bid_size: Decimal,

    /// Ask size
    pub ask_size: Decimal,

    /// Bid implied volatility
    pub bid_implied_volatility: Decimal,

    /// Ask implied volatility
    pub ask_implied_volatility: Decimal,

    /// Mark implied volatility
    pub mark_implied_volatility: Decimal,

    /// Bid price
    pub bid_price: Decimal,

    /// Ask price
    pub ask_price: Decimal,

    /// Mark price
    pub mark_price: Decimal,

    /// Last price
    pub last_price: Decimal,

    /// Underlying price
    pub underlying_price: Decimal,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,
}

impl Greeks {
    /// Create a new Greeks
    pub fn new(symbol: impl Into<String>) -> Self {
        Greeks {
            symbol: symbol.into(),
            timestamp: None,
            datetime: None,
            delta: Decimal::ZERO,
            gamma: Decimal::ZERO,
            theta: Decimal::ZERO,
            vega: Decimal::ZERO,
            rho: Decimal::ZERO,
            vanna: None,
            volga: None,
            charm: None,
            bid_size: Decimal::ZERO,
            ask_size: Decimal::ZERO,
            bid_implied_volatility: Decimal::ZERO,
            ask_implied_volatility: Decimal::ZERO,
            mark_implied_volatility: Decimal::ZERO,
            bid_price: Decimal::ZERO,
            ask_price: Decimal::ZERO,
            mark_price: Decimal::ZERO,
            last_price: Decimal::ZERO,
            underlying_price: Decimal::ZERO,
            info: Value::Null,
        }
    }
}

/// Long/Short ratio for a market
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongShortRatio {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Timeframe
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeframe: Option<String>,

    /// Long/Short ratio (long accounts / short accounts)
    pub long_short_ratio: Decimal,
}

impl LongShortRatio {
    /// Create a new LongShortRatio
    pub fn new(symbol: impl Into<String>, ratio: Decimal) -> Self {
        LongShortRatio {
            info: Value::Null,
            symbol: symbol.into(),
            timestamp: None,
            datetime: None,
            timeframe: None,
            long_short_ratio: ratio,
        }
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set timeframe
    pub fn with_timeframe(mut self, timeframe: impl Into<String>) -> Self {
        self.timeframe = Some(timeframe.into());
        self
    }

    /// Calculate long percentage
    pub fn long_percentage(&self) -> Decimal {
        let ratio = self.long_short_ratio;
        if ratio.is_zero() {
            return Decimal::ZERO;
        }
        ratio / (ratio + Decimal::ONE) * Decimal::from(100)
    }

    /// Calculate short percentage
    pub fn short_percentage(&self) -> Decimal {
        Decimal::from(100) - self.long_percentage()
    }
}

/// Last price information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastPrice {
    /// Trading symbol
    pub symbol: String,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Last trade price
    pub price: Decimal,

    /// Last trade side
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,
}

impl LastPrice {
    /// Create a new LastPrice
    pub fn new(symbol: impl Into<String>, price: Decimal) -> Self {
        LastPrice {
            symbol: symbol.into(),
            timestamp: None,
            datetime: None,
            price,
            side: None,
            info: Value::Null,
        }
    }
}

/// Conversion (currency exchange) record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversion {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Conversion ID
    pub id: String,

    /// Source currency
    pub from_currency: String,

    /// Source amount
    pub from_amount: Decimal,

    /// Target currency
    pub to_currency: String,

    /// Target amount
    pub to_amount: Decimal,

    /// Exchange rate
    pub price: Decimal,

    /// Fee
    pub fee: Decimal,
}

impl Conversion {
    /// Create a new Conversion
    pub fn new(
        id: impl Into<String>,
        from_currency: impl Into<String>,
        from_amount: Decimal,
        to_currency: impl Into<String>,
        to_amount: Decimal,
        price: Decimal,
    ) -> Self {
        Conversion {
            info: Value::Null,
            timestamp: None,
            datetime: None,
            id: id.into(),
            from_currency: from_currency.into(),
            from_amount,
            to_currency: to_currency.into(),
            to_amount,
            price,
            fee: Decimal::ZERO,
        }
    }
}

/// Option chain (options by symbol)
pub type OptionChain = HashMap<String, OptionContract>;

/// Last prices by symbol
pub type LastPrices = HashMap<String, LastPrice>;

/// Option settlement information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settlement {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Settlement ID
    pub id: String,

    /// Trading symbol
    pub symbol: String,

    /// Unix timestamp in milliseconds
    pub timestamp: i64,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Settlement price
    pub settlement_price: Decimal,

    /// Index price at settlement
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_price: Option<Decimal>,

    /// Settlement type (delivery, cash)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_type: Option<String>,

    /// Strike price (for options)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strike: Option<Decimal>,

    /// Option type (call, put)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_type: Option<String>,
}

impl Settlement {
    /// Create a new Settlement
    pub fn new(
        id: impl Into<String>,
        symbol: impl Into<String>,
        timestamp: i64,
        settlement_price: Decimal,
    ) -> Self {
        Settlement {
            info: Value::Null,
            id: id.into(),
            symbol: symbol.into(),
            timestamp,
            datetime: None,
            settlement_price,
            index_price: None,
            settlement_type: None,
            strike: None,
            option_type: None,
        }
    }
}

/// Historical volatility data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityHistory {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Unix timestamp in milliseconds
    pub timestamp: i64,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Underlying asset symbol
    pub symbol: String,

    /// Implied volatility (annualized, as decimal e.g., 0.65 = 65%)
    pub implied_volatility: Decimal,

    /// Historical/realized volatility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub historical_volatility: Option<Decimal>,

    /// Forward volatility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_volatility: Option<Decimal>,

    /// Index price at the time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_price: Option<Decimal>,
}

impl VolatilityHistory {
    /// Create a new VolatilityHistory
    pub fn new(symbol: impl Into<String>, timestamp: i64, implied_volatility: Decimal) -> Self {
        VolatilityHistory {
            info: Value::Null,
            timestamp,
            datetime: None,
            symbol: symbol.into(),
            implied_volatility,
            historical_volatility: None,
            forward_volatility: None,
            index_price: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_long_short_ratio() {
        // Ratio of 1.5 means 1.5 longs for every 1 short
        let ratio = LongShortRatio::new("BTC/USDT:USDT", dec!(1.5)).with_timeframe("5m");

        assert_eq!(ratio.symbol, "BTC/USDT:USDT");
        assert_eq!(ratio.long_short_ratio, dec!(1.5));
        // 1.5 / (1.5 + 1) * 100 = 60%
        assert_eq!(ratio.long_percentage(), dec!(60));
        assert_eq!(ratio.short_percentage(), dec!(40));
    }

    #[test]
    fn test_last_price() {
        let price = LastPrice::new("BTC/USDT", dec!(50000));

        assert_eq!(price.symbol, "BTC/USDT");
        assert_eq!(price.price, dec!(50000));
    }

    #[test]
    fn test_conversion() {
        let conv = Conversion::new("conv123", "BTC", dec!(1), "USDT", dec!(50000), dec!(50000));

        assert_eq!(conv.from_currency, "BTC");
        assert_eq!(conv.to_currency, "USDT");
        assert_eq!(conv.from_amount, dec!(1));
        assert_eq!(conv.to_amount, dec!(50000));
    }
}
