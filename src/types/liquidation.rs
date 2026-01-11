//! Liquidation Types
//!
//! Represents liquidation events in futures markets

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::order::OrderSide;

/// Liquidation event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Liquidation {
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

    /// Liquidation price
    pub price: Decimal,

    /// Base currency value liquidated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_value: Option<Decimal>,

    /// Quote currency value liquidated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_value: Option<Decimal>,

    /// Number of contracts liquidated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contracts: Option<Decimal>,

    /// Contract size
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_size: Option<Decimal>,

    /// Side that was liquidated (long liquidation = sell, short liquidation = buy)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<OrderSide>,
}

impl Liquidation {
    /// Create a new Liquidation
    pub fn new(symbol: impl Into<String>, price: Decimal) -> Self {
        Liquidation {
            info: Value::Null,
            symbol: symbol.into(),
            timestamp: None,
            datetime: None,
            price,
            base_value: None,
            quote_value: None,
            contracts: None,
            contract_size: None,
            side: None,
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

    /// Set base value
    pub fn with_base_value(mut self, value: Decimal) -> Self {
        self.base_value = Some(value);
        self
    }

    /// Set quote value
    pub fn with_quote_value(mut self, value: Decimal) -> Self {
        self.quote_value = Some(value);
        self
    }

    /// Set contracts
    pub fn with_contracts(mut self, contracts: Decimal) -> Self {
        self.contracts = Some(contracts);
        self
    }

    /// Set side
    pub fn with_side(mut self, side: OrderSide) -> Self {
        self.side = Some(side);
        self
    }

    /// Check if this is a long liquidation
    pub fn is_long_liquidation(&self) -> bool {
        // Long liquidation = forced sell
        matches!(self.side, Some(OrderSide::Sell))
    }

    /// Check if this is a short liquidation
    pub fn is_short_liquidation(&self) -> bool {
        // Short liquidation = forced buy
        matches!(self.side, Some(OrderSide::Buy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_liquidation_creation() {
        let liq = Liquidation::new("BTC/USDT:USDT", dec!(48000))
            .with_contracts(dec!(100))
            .with_quote_value(dec!(4800000))
            .with_side(OrderSide::Sell)
            .with_timestamp(1700000000000);

        assert_eq!(liq.symbol, "BTC/USDT:USDT");
        assert_eq!(liq.price, dec!(48000));
        assert_eq!(liq.contracts, Some(dec!(100)));
        assert!(liq.is_long_liquidation());
        assert!(!liq.is_short_liquidation());
    }

    #[test]
    fn test_short_liquidation() {
        let liq = Liquidation::new("ETH/USDT:USDT", dec!(3200)).with_side(OrderSide::Buy);

        assert!(liq.is_short_liquidation());
        assert!(!liq.is_long_liquidation());
    }
}
