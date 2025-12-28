//! Leverage Types
//!
//! Represents leverage settings and tiers for futures trading

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use super::position::MarginMode;

/// Leverage settings for a symbol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leverage {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Trading symbol
    pub symbol: String,

    /// Margin mode
    pub margin_mode: MarginMode,

    /// Long position leverage
    pub long_leverage: Decimal,

    /// Short position leverage
    pub short_leverage: Decimal,
}

impl Leverage {
    /// Create a new Leverage
    pub fn new(
        symbol: impl Into<String>,
        margin_mode: MarginMode,
        long_leverage: Decimal,
        short_leverage: Decimal,
    ) -> Self {
        Leverage {
            info: Value::Null,
            symbol: symbol.into(),
            margin_mode,
            long_leverage,
            short_leverage,
        }
    }

    /// Create symmetric leverage (same for long and short)
    pub fn symmetric(symbol: impl Into<String>, margin_mode: MarginMode, leverage: Decimal) -> Self {
        Self::new(symbol, margin_mode, leverage, leverage)
    }
}

/// Leverage tier for tiered margin systems
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeverageTier {
    /// Tier number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<u32>,

    /// Trading symbol
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,

    /// Currency code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,

    /// Minimum notional value for this tier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_notional: Option<Decimal>,

    /// Maximum notional value for this tier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_notional: Option<Decimal>,

    /// Maintenance margin rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintenance_margin_rate: Option<Decimal>,

    /// Maximum leverage for this tier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_leverage: Option<Decimal>,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,
}

impl LeverageTier {
    /// Create a new LeverageTier
    pub fn new(tier: u32) -> Self {
        LeverageTier {
            tier: Some(tier),
            symbol: None,
            currency: None,
            min_notional: None,
            max_notional: None,
            maintenance_margin_rate: None,
            max_leverage: None,
            info: Value::Null,
        }
    }

    /// Set symbol
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    /// Set notional range
    pub fn with_notional_range(mut self, min: Decimal, max: Decimal) -> Self {
        self.min_notional = Some(min);
        self.max_notional = Some(max);
        self
    }

    /// Set maintenance margin rate
    pub fn with_maintenance_margin_rate(mut self, rate: Decimal) -> Self {
        self.maintenance_margin_rate = Some(rate);
        self
    }

    /// Set max leverage
    pub fn with_max_leverage(mut self, leverage: Decimal) -> Self {
        self.max_leverage = Some(leverage);
        self
    }
}

/// Collection of leverage settings by symbol
pub type Leverages = HashMap<String, Leverage>;

/// Collection of leverage tiers by symbol
pub type LeverageTiers = HashMap<String, Vec<LeverageTier>>;

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_leverage_creation() {
        let leverage = Leverage::new(
            "BTC/USDT:USDT",
            MarginMode::Cross,
            dec!(20),
            dec!(20),
        );

        assert_eq!(leverage.symbol, "BTC/USDT:USDT");
        assert_eq!(leverage.long_leverage, dec!(20));
        assert_eq!(leverage.short_leverage, dec!(20));
    }

    #[test]
    fn test_symmetric_leverage() {
        let leverage = Leverage::symmetric("ETH/USDT:USDT", MarginMode::Isolated, dec!(10));

        assert_eq!(leverage.long_leverage, dec!(10));
        assert_eq!(leverage.short_leverage, dec!(10));
    }

    #[test]
    fn test_leverage_tier() {
        let tier = LeverageTier::new(1)
            .with_symbol("BTC/USDT:USDT")
            .with_notional_range(dec!(0), dec!(50000))
            .with_maintenance_margin_rate(dec!(0.004))
            .with_max_leverage(dec!(125));

        assert_eq!(tier.tier, Some(1));
        assert_eq!(tier.min_notional, Some(dec!(0)));
        assert_eq!(tier.max_notional, Some(dec!(50000)));
        assert_eq!(tier.max_leverage, Some(dec!(125)));
    }
}
