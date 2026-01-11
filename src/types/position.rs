//! Position Type
//!
//! Represents a futures/derivatives position

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Position side
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum PositionSide {
    Long,
    Short,
    #[serde(other)]
    #[default]
    Unknown,
}

/// Margin mode for position
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MarginMode {
    Isolated,
    Cross,
    #[serde(other)]
    #[default]
    Unknown,
}

/// Position mode (hedged vs one-way)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum PositionMode {
    /// Hedge mode - can have both long and short positions simultaneously
    Hedged,
    /// One-way mode - only one position direction at a time
    OneWay,
    #[serde(other)]
    #[default]
    Unknown,
}

/// Position mode information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionModeInfo {
    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Position mode (hedged or one-way)
    pub mode: PositionMode,
}

impl PositionModeInfo {
    pub fn new(mode: PositionMode) -> Self {
        PositionModeInfo {
            info: Value::Null,
            mode,
        }
    }

    /// Set raw exchange response
    pub fn with_info(mut self, info: Value) -> Self {
        self.info = info;
        self
    }

    pub fn is_hedged(&self) -> bool {
        matches!(self.mode, PositionMode::Hedged)
    }

    pub fn is_one_way(&self) -> bool {
        matches!(self.mode, PositionMode::OneWay)
    }
}

/// Futures/derivatives position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Trading symbol
    pub symbol: String,

    /// Position ID (exchange-specific)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Raw exchange response
    #[serde(default)]
    pub info: Value,

    /// Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,

    /// ISO8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,

    /// Number of contracts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contracts: Option<Decimal>,

    /// Contract size
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract_size: Option<Decimal>,

    /// Position side
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<PositionSide>,

    /// Notional value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notional: Option<Decimal>,

    /// Leverage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leverage: Option<Decimal>,

    /// Unrealized profit/loss
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl: Option<Decimal>,

    /// Realized profit/loss
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_pnl: Option<Decimal>,

    /// Collateral amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collateral: Option<Decimal>,

    /// Entry price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_price: Option<Decimal>,

    /// Mark price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<Decimal>,

    /// Liquidation price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub liquidation_price: Option<Decimal>,

    /// Margin mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_mode: Option<MarginMode>,

    /// Whether position is hedged
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hedged: Option<bool>,

    /// Maintenance margin
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintenance_margin: Option<Decimal>,

    /// Maintenance margin percentage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintenance_margin_percentage: Option<Decimal>,

    /// Initial margin
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_margin: Option<Decimal>,

    /// Initial margin percentage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_margin_percentage: Option<Decimal>,

    /// Margin ratio
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_ratio: Option<Decimal>,

    /// Last update timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_timestamp: Option<i64>,

    /// Last price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_price: Option<Decimal>,

    /// Stop loss price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_loss_price: Option<Decimal>,

    /// Take profit price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit_price: Option<Decimal>,

    /// Percentage profit/loss
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<Decimal>,
}

impl Position {
    /// Create a new Position with required fields
    pub fn new(symbol: impl Into<String>) -> Self {
        Position {
            symbol: symbol.into(),
            id: None,
            info: Value::Null,
            timestamp: None,
            datetime: None,
            contracts: None,
            contract_size: None,
            side: None,
            notional: None,
            leverage: None,
            unrealized_pnl: None,
            realized_pnl: None,
            collateral: None,
            entry_price: None,
            mark_price: None,
            liquidation_price: None,
            margin_mode: None,
            hedged: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            initial_margin: None,
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: None,
            last_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
        }
    }

    /// Set position ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set position side
    pub fn with_side(mut self, side: PositionSide) -> Self {
        self.side = Some(side);
        self
    }

    /// Set contracts
    pub fn with_contracts(mut self, contracts: Decimal) -> Self {
        self.contracts = Some(contracts);
        self
    }

    /// Set entry price
    pub fn with_entry_price(mut self, price: Decimal) -> Self {
        self.entry_price = Some(price);
        self
    }

    /// Set leverage
    pub fn with_leverage(mut self, leverage: Decimal) -> Self {
        self.leverage = Some(leverage);
        self
    }

    /// Set unrealized PnL
    pub fn with_unrealized_pnl(mut self, pnl: Decimal) -> Self {
        self.unrealized_pnl = Some(pnl);
        self
    }

    /// Set margin mode
    pub fn with_margin_mode(mut self, mode: MarginMode) -> Self {
        self.margin_mode = Some(mode);
        self
    }

    /// Check if position is long
    pub fn is_long(&self) -> bool {
        matches!(self.side, Some(PositionSide::Long))
    }

    /// Check if position is short
    pub fn is_short(&self) -> bool {
        matches!(self.side, Some(PositionSide::Short))
    }

    /// Calculate position value (contracts * contract_size * mark_price)
    pub fn value(&self) -> Option<Decimal> {
        let contracts = self.contracts?;
        let contract_size = self.contract_size.unwrap_or(Decimal::ONE);
        let mark_price = self.mark_price?;
        Some(contracts * contract_size * mark_price)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_position_creation() {
        let position = Position::new("BTC/USDT:USDT")
            .with_side(PositionSide::Long)
            .with_contracts(dec!(1.5))
            .with_entry_price(dec!(50000))
            .with_leverage(dec!(10))
            .with_margin_mode(MarginMode::Isolated);

        assert_eq!(position.symbol, "BTC/USDT:USDT");
        assert!(position.is_long());
        assert!(!position.is_short());
        assert_eq!(position.contracts, Some(dec!(1.5)));
        assert_eq!(position.leverage, Some(dec!(10)));
    }

    #[test]
    fn test_position_value() {
        let mut position = Position::new("BTC/USDT:USDT");
        position.contracts = Some(dec!(2));
        position.contract_size = Some(dec!(0.001)); // 0.001 BTC per contract
        position.mark_price = Some(dec!(50000));

        assert_eq!(position.value(), Some(dec!(100))); // 2 * 0.001 * 50000 = 100
    }
}
