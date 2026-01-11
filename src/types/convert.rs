//! Convert Types
//!
//! Types for currency conversion (swap) functionality

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Convert quote (price quote for currency conversion)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertQuote {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Quote ID
    pub id: String,

    /// From currency code
    pub from_currency: String,

    /// To currency code
    pub to_currency: String,

    /// Amount to convert
    pub amount: Decimal,

    /// Conversion price/rate
    pub price: Decimal,

    /// Result amount after conversion
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_amount: Option<Decimal>,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Quote expiry timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expire_timestamp: Option<i64>,
}

impl ConvertQuote {
    /// Create a new ConvertQuote
    pub fn new(
        id: impl Into<String>,
        from_currency: impl Into<String>,
        to_currency: impl Into<String>,
        amount: Decimal,
        price: Decimal,
    ) -> Self {
        ConvertQuote {
            info: Value::Null,
            id: id.into(),
            from_currency: from_currency.into(),
            to_currency: to_currency.into(),
            amount,
            price,
            to_amount: None,
            timestamp: None,
            datetime: None,
            expire_timestamp: None,
        }
    }
}

/// Convert trade (executed conversion)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertTrade {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trade ID
    pub id: String,

    /// From currency code
    pub from_currency: String,

    /// To currency code
    pub to_currency: String,

    /// Amount converted from
    pub from_amount: Decimal,

    /// Amount converted to
    pub to_amount: Decimal,

    /// Conversion price/rate
    pub price: Decimal,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Trade status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl ConvertTrade {
    /// Create a new ConvertTrade
    pub fn new(
        id: impl Into<String>,
        from_currency: impl Into<String>,
        to_currency: impl Into<String>,
        from_amount: Decimal,
        to_amount: Decimal,
        price: Decimal,
    ) -> Self {
        ConvertTrade {
            info: Value::Null,
            id: id.into(),
            from_currency: from_currency.into(),
            to_currency: to_currency.into(),
            from_amount,
            to_amount,
            price,
            timestamp: None,
            datetime: None,
            status: None,
        }
    }
}

/// Convert currency info (supported currencies for conversion)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertCurrencyPair {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// From currency code
    pub from_currency: String,

    /// To currency code
    pub to_currency: String,

    /// Minimum amount to convert
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_amount: Option<Decimal>,

    /// Maximum amount to convert
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_amount: Option<Decimal>,
}

impl ConvertCurrencyPair {
    /// Create a new ConvertCurrencyPair
    pub fn new(from_currency: impl Into<String>, to_currency: impl Into<String>) -> Self {
        ConvertCurrencyPair {
            info: Value::Null,
            from_currency: from_currency.into(),
            to_currency: to_currency.into(),
            min_amount: None,
            max_amount: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_convert_quote() {
        let quote = ConvertQuote::new("quote123", "BTC", "USDT", dec!(0.1), dec!(50000));

        assert_eq!(quote.id, "quote123");
        assert_eq!(quote.from_currency, "BTC");
        assert_eq!(quote.to_currency, "USDT");
        assert_eq!(quote.amount, dec!(0.1));
        assert_eq!(quote.price, dec!(50000));
    }

    #[test]
    fn test_convert_trade() {
        let trade = ConvertTrade::new(
            "trade123",
            "BTC",
            "USDT",
            dec!(0.1),
            dec!(5000),
            dec!(50000),
        );

        assert_eq!(trade.id, "trade123");
        assert_eq!(trade.from_currency, "BTC");
        assert_eq!(trade.to_currency, "USDT");
        assert_eq!(trade.from_amount, dec!(0.1));
        assert_eq!(trade.to_amount, dec!(5000));
    }
}
