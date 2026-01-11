//! Derivatives API Trait
//!
//! Futures and perpetual contract operations.

use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{
    FundingRate, FundingRateHistory, Leverage, LeverageTier, Liquidation,
    LongShortRatio, OpenInterest, Order, OrderSide, Position, PositionModeInfo,
    Ticker, Timeframe, OHLCV,
};

/// Derivatives (Futures/Perpetual) API
///
/// Operations for futures and perpetual contracts:
/// - Position management
/// - Leverage settings
/// - Funding rates
/// - Mark/Index prices
/// - Open interest
/// - Liquidations
#[async_trait]
pub trait DerivativesApi: Send + Sync {
    // ========================================================================
    // Position Management
    // ========================================================================

    /// Fetch a single position
    async fn fetch_position(&self, symbol: &str) -> CcxtResult<Position> {
        let _ = symbol;
        not_supported!("fetchPosition")
    }

    /// Fetch all positions
    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        let _ = symbols;
        not_supported!("fetchPositions")
    }

    /// Close a position
    async fn close_position(&self, symbol: &str, side: Option<OrderSide>) -> CcxtResult<Order> {
        let _ = (symbol, side);
        not_supported!("closePosition")
    }

    /// Close all positions
    async fn close_all_positions(&self) -> CcxtResult<Vec<Order>> {
        not_supported!("closeAllPositions")
    }

    /// Fetch position history for a symbol
    async fn fetch_position_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchPositionHistory")
    }

    /// Fetch all position history
    async fn fetch_positions_history(
        &self,
        symbols: Option<&[&str]>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Position>> {
        let _ = (symbols, since, limit);
        not_supported!("fetchPositionsHistory")
    }

    // ========================================================================
    // Position Mode
    // ========================================================================

    /// Set position mode (hedged/one-way)
    async fn set_position_mode(
        &self,
        hedged: bool,
        symbol: Option<&str>,
    ) -> CcxtResult<PositionModeInfo> {
        let _ = (hedged, symbol);
        not_supported!("setPositionMode")
    }

    /// Fetch current position mode
    async fn fetch_position_mode(&self, symbol: Option<&str>) -> CcxtResult<PositionModeInfo> {
        let _ = symbol;
        not_supported!("fetchPositionMode")
    }

    // ========================================================================
    // Leverage
    // ========================================================================

    /// Set leverage for a symbol
    async fn set_leverage(&self, leverage: Decimal, symbol: &str) -> CcxtResult<Leverage> {
        let _ = (leverage, symbol);
        not_supported!("setLeverage")
    }

    /// Fetch current leverage for a symbol
    async fn fetch_leverage(&self, symbol: &str) -> CcxtResult<Leverage> {
        let _ = symbol;
        not_supported!("fetchLeverage")
    }

    /// Fetch leverage tiers for symbols
    async fn fetch_leverage_tiers(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Vec<LeverageTier>>> {
        let _ = symbols;
        not_supported!("fetchLeverageTiers")
    }

    // ========================================================================
    // Funding Rates
    // ========================================================================

    /// Fetch current funding rate
    async fn fetch_funding_rate(&self, symbol: &str) -> CcxtResult<FundingRate> {
        let _ = symbol;
        not_supported!("fetchFundingRate")
    }

    /// Fetch funding rates for multiple symbols
    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        let _ = symbols;
        not_supported!("fetchFundingRates")
    }

    /// Fetch funding rate history
    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchFundingRateHistory")
    }

    /// Fetch funding history (payments received/paid)
    async fn fetch_funding_history(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::FundingHistory>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchFundingHistory")
    }

    // ========================================================================
    // Mark/Index Prices
    // ========================================================================

    /// Fetch mark price for a symbol
    async fn fetch_mark_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let _ = symbol;
        not_supported!("fetchMarkPrice")
    }

    /// Fetch mark prices for multiple symbols
    async fn fetch_mark_prices(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let _ = symbols;
        not_supported!("fetchMarkPrices")
    }

    /// Fetch index price for a symbol
    async fn fetch_index_price(&self, symbol: &str) -> CcxtResult<Ticker> {
        let _ = symbol;
        not_supported!("fetchIndexPrice")
    }

    /// Fetch mark price OHLCV
    async fn fetch_mark_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let _ = (symbol, timeframe, since, limit);
        not_supported!("fetchMarkOHLCV")
    }

    /// Fetch index price OHLCV
    async fn fetch_index_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let _ = (symbol, timeframe, since, limit);
        not_supported!("fetchIndexOHLCV")
    }

    // ========================================================================
    // Open Interest
    // ========================================================================

    /// Fetch open interest for a symbol
    async fn fetch_open_interest(&self, symbol: &str) -> CcxtResult<OpenInterest> {
        let _ = symbol;
        not_supported!("fetchOpenInterest")
    }

    /// Fetch open interest history
    async fn fetch_open_interest_history(
        &self,
        symbol: &str,
        timeframe: Option<Timeframe>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OpenInterest>> {
        let _ = (symbol, timeframe, since, limit);
        not_supported!("fetchOpenInterestHistory")
    }

    // ========================================================================
    // Liquidations
    // ========================================================================

    /// Fetch liquidations for a symbol
    async fn fetch_liquidations(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchLiquidations")
    }

    /// Fetch my liquidations
    async fn fetch_my_liquidations(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Liquidation>> {
        let _ = (symbol, since, limit);
        not_supported!("fetchMyLiquidations")
    }

    // ========================================================================
    // Long/Short Ratio
    // ========================================================================

    /// Fetch long/short ratio
    async fn fetch_long_short_ratio(
        &self,
        symbol: &str,
        timeframe: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<LongShortRatio>> {
        let _ = (symbol, timeframe, since, limit);
        not_supported!("fetchLongShortRatio")
    }
}
