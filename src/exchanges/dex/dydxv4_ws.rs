//! dYdX v4 WebSocket Implementation
//!
//! Alias for DydxWs (which already implements v4 WebSocket API)
//!
//! This wrapper provides API naming consistency with the REST API
//! where DydxV4 is separate from Dydx (v3).

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use crate::types::{Timeframe, WsExchange, WsMessage};

use super::dydx_ws::DydxWs;

/// dYdX v4 WebSocket client
///
/// This is an alias for `DydxWs` which already implements the v4 WebSocket API.
/// The wrapper provides naming consistency with the REST API where `DydxV4`
/// is the Cosmos SDK-based v4 implementation.
///
/// # Public Channels
/// - `v4_markets` - Market ticker updates
/// - `v4_orderbook` - Order book updates
/// - `v4_trades` - Trade updates
/// - `v4_candles` - OHLCV candle updates
///
/// # Private Channels (requires subaccount address)
/// - `v4_subaccounts` - Account, position, order, and fill updates
///
/// # Example
///
/// ```rust,ignore
/// use ccxt_rust::exchanges::DydxV4Ws;
///
/// // Public WebSocket (no authentication)
/// let ws = DydxV4Ws::new();
/// let mut rx = ws.watch_ticker("BTC/USD").await?;
///
/// // Private WebSocket (with subaccount for orders, positions)
/// let ws = DydxV4Ws::with_subaccount("dydx1abc...", 0, false);
/// let mut rx = ws.watch_orders().await?;
/// ```
pub struct DydxV4Ws(DydxWs);

impl DydxV4Ws {
    /// Create new dYdX v4 WebSocket client (Mainnet)
    pub fn new() -> Self {
        Self(DydxWs::new())
    }

    /// Create new dYdX v4 WebSocket client for testnet
    pub fn testnet() -> Self {
        Self(DydxWs::testnet())
    }

    /// Create dYdX v4 WebSocket client with subaccount for private channels
    ///
    /// # Arguments
    ///
    /// * `address` - dYdX address (e.g., "dydx1...")
    /// * `subaccount_number` - Subaccount number (default: 0)
    /// * `testnet` - Use testnet endpoints
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ws = DydxV4Ws::with_subaccount("dydx1abc123...", 0, false);
    /// ```
    pub fn with_subaccount(address: &str, subaccount_number: u32, testnet: bool) -> Self {
        Self(DydxWs::with_subaccount(address, subaccount_number, testnet))
    }

    /// Subscribe to order updates (requires subaccount)
    ///
    /// Returns a stream of order status updates including new orders,
    /// fills, and cancellations.
    pub async fn watch_orders(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_orders().await
    }

    /// Subscribe to position updates (requires subaccount)
    ///
    /// Returns a stream of position updates including size changes,
    /// PnL updates, and liquidations.
    pub async fn watch_positions(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_positions().await
    }

    /// Subscribe to fill/trade updates (requires subaccount)
    ///
    /// Returns a stream of fill updates for your orders.
    pub async fn watch_my_trades(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_my_trades().await
    }

    /// Subscribe to all private updates (orders, positions, fills)
    ///
    /// This is the most efficient way to get all account updates
    /// as they all come from the same v4_subaccounts channel.
    pub async fn watch_account(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_account().await
    }

    /// Check if this instance has subaccount configured for private channels
    pub fn has_subaccount(&self) -> bool {
        self.0.has_subaccount()
    }

    /// Get the configured subaccount ID
    pub fn get_subaccount_id(&self) -> Option<&str> {
        self.0.get_subaccount_id()
    }
}

impl Default for DydxV4Ws {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for DydxV4Ws {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        self.0.ws_connect().await
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        self.0.ws_close().await
    }

    async fn ws_is_connected(&self) -> bool {
        self.0.ws_is_connected().await
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_ticker(symbol).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_tickers(symbols).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_order_book(symbol, limit).await
    }

    async fn watch_order_book_for_symbols(&self, symbols: &[&str], limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_order_book_for_symbols(symbols, limit).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_trades(symbol).await
    }

    async fn watch_trades_for_symbols(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_trades_for_symbols(symbols).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_ohlcv(symbol, timeframe).await
    }

    async fn watch_ohlcv_for_symbols(&self, symbols: &[&str], timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_ohlcv_for_symbols(symbols, timeframe).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dydxv4_ws_creation() {
        let ws = DydxV4Ws::new();
        assert!(!ws.has_subaccount());
    }

    #[test]
    fn test_dydxv4_ws_testnet() {
        let _ws = DydxV4Ws::testnet();
    }

    #[test]
    fn test_dydxv4_ws_with_subaccount() {
        let ws = DydxV4Ws::with_subaccount("dydx1abc123", 0, false);
        assert!(ws.has_subaccount());
        assert_eq!(ws.get_subaccount_id(), Some("dydx1abc123/0"));
    }

    #[test]
    fn test_dydxv4_ws_with_subaccount_testnet() {
        let ws = DydxV4Ws::with_subaccount("dydx1xyz789", 1, true);
        assert!(ws.has_subaccount());
        assert_eq!(ws.get_subaccount_id(), Some("dydx1xyz789/1"));
    }

    #[test]
    fn test_default() {
        let ws = DydxV4Ws::default();
        assert!(!ws.has_subaccount());
    }
}
