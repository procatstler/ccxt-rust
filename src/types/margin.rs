//! Margin Trading Types
//!
//! Types for margin trading operations

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use super::position::MarginMode;

/// Borrow interest information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BorrowInterest {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol (for isolated margin)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,

    /// Interest amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interest: Option<Decimal>,

    /// Interest rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interest_rate: Option<Decimal>,

    /// Amount borrowed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_borrowed: Option<Decimal>,

    /// Margin mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_mode: Option<MarginMode>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl BorrowInterest {
    /// Create a new BorrowInterest
    pub fn new() -> Self {
        BorrowInterest {
            info: Value::Null,
            symbol: None,
            currency: None,
            interest: None,
            interest_rate: None,
            amount_borrowed: None,
            margin_mode: None,
            timestamp: None,
            datetime: None,
        }
    }

    /// Set currency
    pub fn with_currency(mut self, currency: impl Into<String>) -> Self {
        self.currency = Some(currency.into());
        self
    }

    /// Set interest rate
    pub fn with_interest_rate(mut self, rate: Decimal) -> Self {
        self.interest_rate = Some(rate);
        self
    }

    /// Set amount borrowed
    pub fn with_amount_borrowed(mut self, amount: Decimal) -> Self {
        self.amount_borrowed = Some(amount);
        self
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self.datetime =
            chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339());
        self
    }
}

impl Default for BorrowInterest {
    fn default() -> Self {
        Self::new()
    }
}

/// Margin loan information (for borrow/repay operations)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginLoan {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Transaction ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,

    /// Loan amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Decimal>,

    /// Trading symbol (for isolated margin)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl MarginLoan {
    /// Create a new MarginLoan
    pub fn new() -> Self {
        MarginLoan {
            info: Value::Null,
            id: None,
            currency: None,
            amount: None,
            symbol: None,
            timestamp: None,
            datetime: None,
        }
    }

    /// Set ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set currency
    pub fn with_currency(mut self, currency: impl Into<String>) -> Self {
        self.currency = Some(currency.into());
        self
    }

    /// Set amount
    pub fn with_amount(mut self, amount: Decimal) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Set symbol
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    /// Set timestamp
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.timestamp = Some(timestamp);
        self.datetime =
            chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339());
        self
    }

    /// Set info
    pub fn with_info(mut self, info: Value) -> Self {
        self.info = info;
        self
    }
}

impl Default for MarginLoan {
    fn default() -> Self {
        Self::new()
    }
}

/// Cross margin borrow rate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossBorrowRate {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,

    /// Borrow rate
    pub rate: Decimal,

    /// Period in milliseconds (e.g., hourly, daily)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<i64>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl CrossBorrowRate {
    /// Create a new CrossBorrowRate
    pub fn new(rate: Decimal) -> Self {
        CrossBorrowRate {
            info: Value::Null,
            currency: None,
            rate,
            period: None,
            timestamp: None,
            datetime: None,
        }
    }

    /// Set currency
    pub fn with_currency(mut self, currency: impl Into<String>) -> Self {
        self.currency = Some(currency.into());
        self
    }

    /// Set period
    pub fn with_period(mut self, period: i64) -> Self {
        self.period = Some(period);
        self
    }
}

/// Isolated margin borrow rate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolatedBorrowRate {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Base currency
    pub base: String,

    /// Base currency borrow rate
    pub base_rate: Decimal,

    /// Quote currency
    pub quote: String,

    /// Quote currency borrow rate
    pub quote_rate: Decimal,

    /// Period in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<i64>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl IsolatedBorrowRate {
    /// Create a new IsolatedBorrowRate
    pub fn new(
        symbol: impl Into<String>,
        base: impl Into<String>,
        base_rate: Decimal,
        quote: impl Into<String>,
        quote_rate: Decimal,
    ) -> Self {
        IsolatedBorrowRate {
            info: Value::Null,
            symbol: symbol.into(),
            base: base.into(),
            base_rate,
            quote: quote.into(),
            quote_rate,
            period: None,
            timestamp: None,
            datetime: None,
        }
    }
}

/// Margin mode information for a symbol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginModeInfo {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Current margin mode
    pub margin_mode: MarginMode,
}

impl MarginModeInfo {
    /// Create a new MarginModeInfo
    pub fn new(symbol: impl Into<String>, margin_mode: MarginMode) -> Self {
        MarginModeInfo {
            info: Value::Null,
            symbol: symbol.into(),
            margin_mode,
        }
    }
}

/// Margin modification type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MarginModificationType {
    Add,
    Reduce,
    Set,
}

/// Margin modification record
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarginModification {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Modification type
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modification_type: Option<MarginModificationType>,

    /// Margin mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_mode: Option<MarginMode>,

    /// Amount modified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Decimal>,

    /// Total margin after modification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<Decimal>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,

    /// Status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
}

impl MarginModification {
    /// Create a new MarginModification
    pub fn new(symbol: impl Into<String>) -> Self {
        MarginModification {
            info: Value::Null,
            symbol: symbol.into(),
            modification_type: None,
            margin_mode: None,
            amount: None,
            total: None,
            code: None,
            status: None,
            timestamp: None,
            datetime: None,
        }
    }

    /// Set modification type
    pub fn with_type(mut self, modification_type: MarginModificationType) -> Self {
        self.modification_type = Some(modification_type);
        self
    }

    /// Set amount
    pub fn with_amount(mut self, amount: Decimal) -> Self {
        self.amount = Some(amount);
        self
    }
}

/// Collection of cross borrow rates by currency
pub type CrossBorrowRates = HashMap<String, CrossBorrowRate>;

/// Collection of isolated borrow rates by symbol
pub type IsolatedBorrowRates = HashMap<String, IsolatedBorrowRate>;

/// Collection of margin mode info by symbol
pub type MarginModeInfos = HashMap<String, MarginModeInfo>;

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_borrow_interest() {
        let interest = BorrowInterest::new()
            .with_currency("USDT")
            .with_interest_rate(dec!(0.0001))
            .with_amount_borrowed(dec!(10000));

        assert_eq!(interest.currency, Some("USDT".to_string()));
        assert_eq!(interest.interest_rate, Some(dec!(0.0001)));
        assert_eq!(interest.amount_borrowed, Some(dec!(10000)));
    }

    #[test]
    fn test_cross_borrow_rate() {
        let rate = CrossBorrowRate::new(dec!(0.00005))
            .with_currency("BTC")
            .with_period(3600000); // hourly

        assert_eq!(rate.rate, dec!(0.00005));
        assert_eq!(rate.currency, Some("BTC".to_string()));
        assert_eq!(rate.period, Some(3600000));
    }

    #[test]
    fn test_isolated_borrow_rate() {
        let rate = IsolatedBorrowRate::new("BTC/USDT", "BTC", dec!(0.00003), "USDT", dec!(0.00005));

        assert_eq!(rate.symbol, "BTC/USDT");
        assert_eq!(rate.base_rate, dec!(0.00003));
        assert_eq!(rate.quote_rate, dec!(0.00005));
    }

    #[test]
    fn test_margin_modification() {
        let modification = MarginModification::new("BTC/USDT:USDT")
            .with_type(MarginModificationType::Add)
            .with_amount(dec!(1000));

        assert_eq!(modification.symbol, "BTC/USDT:USDT");
        assert_eq!(
            modification.modification_type,
            Some(MarginModificationType::Add)
        );
        assert_eq!(modification.amount, Some(dec!(1000)));
    }
}
