//! Funding Rate Types
//!
//! Represents funding rates for perpetual futures

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Current funding rate information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingRate {
    /// Trading symbol
    pub symbol: String,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Current funding rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding_rate: Option<Decimal>,

    /// Mark price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<Decimal>,

    /// Index price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_price: Option<Decimal>,

    /// Interest rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interest_rate: Option<Decimal>,

    /// Estimated settle price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_settle_price: Option<Decimal>,

    /// Funding timestamp (when funding is applied)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding_timestamp: Option<i64>,

    /// Funding datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding_datetime: Option<String>,

    /// Next funding timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_funding_timestamp: Option<i64>,

    /// Next funding datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_funding_datetime: Option<String>,

    /// Next funding rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_funding_rate: Option<Decimal>,

    /// Previous funding timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_funding_timestamp: Option<i64>,

    /// Previous funding datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_funding_datetime: Option<String>,

    /// Previous funding rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_funding_rate: Option<Decimal>,

    /// Funding interval (e.g., "8h")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
}

impl FundingRate {
    /// Create a new FundingRate
    pub fn new(symbol: impl Into<String>) -> Self {
        FundingRate {
            symbol: symbol.into(),
            info: Value::Null,
            timestamp: None,
            datetime: None,
            funding_rate: None,
            mark_price: None,
            index_price: None,
            interest_rate: None,
            estimated_settle_price: None,
            funding_timestamp: None,
            funding_datetime: None,
            next_funding_timestamp: None,
            next_funding_datetime: None,
            next_funding_rate: None,
            previous_funding_timestamp: None,
            previous_funding_datetime: None,
            previous_funding_rate: None,
            interval: None,
        }
    }

    /// Set funding rate
    pub fn with_funding_rate(mut self, rate: Decimal) -> Self {
        self.funding_rate = Some(rate);
        self
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Set mark price
    pub fn with_mark_price(mut self, price: Decimal) -> Self {
        self.mark_price = Some(price);
        self
    }

    /// Set index price
    pub fn with_index_price(mut self, price: Decimal) -> Self {
        self.index_price = Some(price);
        self
    }

    /// Calculate annual funding rate (assuming 8h intervals)
    pub fn annual_rate(&self) -> Option<Decimal> {
        let rate = self.funding_rate?;
        // 3 funding periods per day * 365 days = 1095
        Some(rate * Decimal::from(1095))
    }
}

/// Historical funding rate entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingRateHistory {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Funding rate
    pub funding_rate: Decimal,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl FundingRateHistory {
    /// Create a new FundingRateHistory
    pub fn new(symbol: impl Into<String>, funding_rate: Decimal) -> Self {
        FundingRateHistory {
            info: Value::Null,
            symbol: symbol.into(),
            funding_rate,
            timestamp: None,
            datetime: None,
        }
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

/// Funding history (funding payments received/paid)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingHistory {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Currency code
    pub code: String,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Funding ID
    pub id: String,

    /// Funding amount (positive = received, negative = paid)
    pub amount: Decimal,
}

impl FundingHistory {
    /// Create a new FundingHistory
    pub fn new(
        symbol: impl Into<String>,
        code: impl Into<String>,
        id: impl Into<String>,
        amount: Decimal,
    ) -> Self {
        FundingHistory {
            info: Value::Null,
            symbol: symbol.into(),
            code: code.into(),
            id: id.into(),
            amount,
            timestamp: None,
            datetime: None,
        }
    }
}

/// Collection of funding rates by symbol
pub type FundingRates = HashMap<String, FundingRate>;

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_funding_rate_creation() {
        let rate = FundingRate::new("BTC/USDT:USDT")
            .with_funding_rate(dec!(0.0001))
            .with_mark_price(dec!(50000))
            .with_index_price(dec!(50010));

        assert_eq!(rate.symbol, "BTC/USDT:USDT");
        assert_eq!(rate.funding_rate, Some(dec!(0.0001)));
        assert_eq!(rate.mark_price, Some(dec!(50000)));
    }

    #[test]
    fn test_annual_rate() {
        let rate = FundingRate::new("BTC/USDT:USDT")
            .with_funding_rate(dec!(0.0001));

        // 0.0001 * 1095 = 0.1095 = 10.95% annual
        assert_eq!(rate.annual_rate(), Some(dec!(0.1095)));
    }

    #[test]
    fn test_funding_rate_history() {
        let history = FundingRateHistory::new("BTC/USDT:USDT", dec!(0.00015))
            .with_timestamp(1700000000000);

        assert_eq!(history.symbol, "BTC/USDT:USDT");
        assert_eq!(history.funding_rate, dec!(0.00015));
        assert_eq!(history.timestamp, Some(1700000000000));
    }
}
