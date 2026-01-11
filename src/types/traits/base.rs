//! Exchange Base Trait
//!
//! Core metadata, market loading, and utility methods that all exchanges must implement.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::errors::CcxtResult;
use crate::types::{
    ExchangeFeatures, ExchangeUrls, Market, SignedRequest, Timeframe,
};

/// Core exchange trait with metadata and utilities
///
/// This trait contains the essential methods that every exchange must implement:
/// - Identity and metadata (id, name, version)
/// - Feature capabilities (has, has_feature)
/// - Market loading and caching
/// - Request signing
#[async_trait]
pub trait ExchangeBase: Send + Sync {
    // ========================================================================
    // Identity & Metadata
    // ========================================================================

    /// Exchange identifier
    fn id(&self) -> crate::types::ExchangeId;

    /// Human-readable exchange name
    fn name(&self) -> &str;

    /// API version
    fn version(&self) -> &str {
        "v1"
    }

    /// Countries where the exchange operates
    fn countries(&self) -> &[&str] {
        &[]
    }

    /// Rate limit in milliseconds between requests
    fn rate_limit(&self) -> u64 {
        1000
    }

    // ========================================================================
    // Feature Capabilities
    // ========================================================================

    /// Get supported features
    fn has(&self) -> &ExchangeFeatures;

    /// Check if a specific feature is supported
    fn has_feature(&self, feature: &str) -> bool {
        let features = self.has();
        match feature {
            "fetchMarkets" => features.fetch_markets,
            "fetchCurrencies" => features.fetch_currencies,
            "fetchTicker" => features.fetch_ticker,
            "fetchTickers" => features.fetch_tickers,
            "fetchOrderBook" => features.fetch_order_book,
            "fetchTrades" => features.fetch_trades,
            "fetchOHLCV" => features.fetch_ohlcv,
            "fetchBalance" => features.fetch_balance,
            "createOrder" => features.create_order,
            "cancelOrder" => features.cancel_order,
            "fetchOrder" => features.fetch_order,
            "fetchOrders" => features.fetch_orders,
            "fetchOpenOrders" => features.fetch_open_orders,
            "fetchClosedOrders" => features.fetch_closed_orders,
            "fetchMyTrades" => features.fetch_my_trades,
            "fetchDeposits" => features.fetch_deposits,
            "fetchWithdrawals" => features.fetch_withdrawals,
            "withdraw" => features.withdraw,
            _ => false,
        }
    }

    /// Exchange URLs (API, docs, website)
    fn urls(&self) -> &ExchangeUrls;

    /// Supported timeframes for OHLCV
    fn timeframes(&self) -> &HashMap<Timeframe, String>;

    // ========================================================================
    // Market Loading
    // ========================================================================

    /// Load markets with optional caching
    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>>;

    /// Fetch all available markets
    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>>;

    // ========================================================================
    // Utilities
    // ========================================================================

    /// Convert unified symbol to exchange-specific market ID
    fn market_id(&self, symbol: &str) -> Option<String>;

    /// Convert exchange-specific market ID to unified symbol
    fn symbol(&self, market_id: &str) -> Option<String>;

    /// Sign a request for authenticated endpoints
    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest;
}
