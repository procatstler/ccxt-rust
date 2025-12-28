//! Open Interest Types
//!
//! Represents open interest data for futures markets

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Open interest data for a futures market
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenInterest {
    /// Trading symbol
    pub symbol: String,

    /// Open interest amount (number of contracts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_interest_amount: Option<Decimal>,

    /// Open interest value (notional value)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_interest_value: Option<Decimal>,

    /// Base currency volume
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_volume: Option<Decimal>,

    /// Quote currency volume
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_volume: Option<Decimal>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,
}

impl OpenInterest {
    /// Create a new OpenInterest
    pub fn new(symbol: impl Into<String>) -> Self {
        OpenInterest {
            symbol: symbol.into(),
            open_interest_amount: None,
            open_interest_value: None,
            base_volume: None,
            quote_volume: None,
            timestamp: None,
            datetime: None,
            info: Value::Null,
        }
    }

    /// Set open interest amount
    pub fn with_amount(mut self, amount: Decimal) -> Self {
        self.open_interest_amount = Some(amount);
        self
    }

    /// Set open interest value
    pub fn with_value(mut self, value: Decimal) -> Self {
        self.open_interest_value = Some(value);
        self
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set datetime
    pub fn with_datetime(mut self, datetime: impl Into<String>) -> Self {
        self.datetime = Some(datetime.into());
        self
    }
}

/// Collection of open interest data by symbol
pub type OpenInterests = HashMap<String, OpenInterest>;

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_open_interest_creation() {
        let oi = OpenInterest::new("BTC/USDT:USDT")
            .with_amount(dec!(50000))
            .with_value(dec!(2500000000))
            .with_timestamp(1700000000000);

        assert_eq!(oi.symbol, "BTC/USDT:USDT");
        assert_eq!(oi.open_interest_amount, Some(dec!(50000)));
        assert_eq!(oi.open_interest_value, Some(dec!(2500000000)));
        assert_eq!(oi.timestamp, Some(1700000000000));
    }
}
