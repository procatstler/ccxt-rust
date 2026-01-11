//! Public Market API Trait
//!
//! Public market data methods that don't require authentication.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::errors::CcxtResult;
use crate::not_supported;
use crate::types::{Currency, OrderBook, Ticker, Timeframe, Trade, OHLCV};

/// Public market data API
///
/// Methods for fetching public market data without authentication:
/// - Currencies and trading pairs
/// - Ticker prices
/// - Order books
/// - Recent trades
/// - OHLCV candlestick data
/// - Exchange status and time
#[async_trait]
pub trait PublicMarketApi: Send + Sync {
    // ========================================================================
    // Currency & Market Info
    // ========================================================================

    /// Fetch available currencies
    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>> {
        not_supported!("fetchCurrencies")
    }

    // ========================================================================
    // Ticker Data
    // ========================================================================

    /// Fetch ticker for a symbol
    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker>;

    /// Fetch tickers for multiple symbols
    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let _ = symbols;
        not_supported!("fetchTickers")
    }

    // ========================================================================
    // Order Book
    // ========================================================================

    /// Fetch order book for a symbol
    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook>;

    // ========================================================================
    // Trades
    // ========================================================================

    /// Fetch recent trades for a symbol
    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>>;

    // ========================================================================
    // OHLCV (Candlesticks)
    // ========================================================================

    /// Fetch OHLCV candlestick data
    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>>;

    // ========================================================================
    // Exchange Status
    // ========================================================================

    /// Fetch server time
    async fn fetch_time(&self) -> CcxtResult<i64> {
        not_supported!("fetchTime")
    }

    /// Fetch exchange status
    async fn fetch_status(&self) -> CcxtResult<crate::types::ExchangeStatus> {
        not_supported!("fetchStatus")
    }

    /// Fetch best bid/ask prices
    async fn fetch_bids_asks(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, crate::types::BidAsk>> {
        let _ = symbols;
        not_supported!("fetchBidsAsks")
    }
}
