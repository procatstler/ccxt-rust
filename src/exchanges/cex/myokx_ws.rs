//! MyOKX WebSocket Implementation
//!
//! Alias for OKX WebSocket

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use crate::types::{Timeframe, WsExchange, WsMessage};

use super::okx_ws::OkxWs;

/// MyOKX WebSocket 클라이언트 (OkxWs 래퍼)
///
/// MyOKX는 OKX와 동일한 WebSocket API를 사용하는 alias입니다.
pub struct MyOkxWs(OkxWs);

impl MyOkxWs {
    /// 새 MyOKX WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self(OkxWs::new())
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: &str, api_secret: &str, passphrase: &str) -> Self {
        Self(OkxWs::with_credentials(api_key, api_secret, passphrase))
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: &str, api_secret: &str, passphrase: &str) {
        self.0.set_credentials(api_key, api_secret, passphrase);
    }
}

impl Default for MyOkxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for MyOkxWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        false
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_ticker(symbol).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_tickers(symbols).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_order_book(symbol, limit).await
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_order_book_for_symbols(symbols, limit).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_trades(symbol).await
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_trades_for_symbols(symbols).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_ohlcv(symbol, timeframe).await
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        symbols: &[&str],
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_ohlcv_for_symbols(symbols, timeframe).await
    }

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_orders(symbol).await
    }

    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_my_trades(symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_balance().await
    }

    async fn watch_positions(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_positions(symbols).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_myokx_ws_creation() {
        let _ws = MyOkxWs::new();
    }

    #[test]
    fn test_with_credentials() {
        let _ws = MyOkxWs::with_credentials("api_key", "api_secret", "passphrase");
        // Credentials are set in inner OkxWs
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = MyOkxWs::new();
        ws.set_credentials("api_key", "api_secret", "passphrase");
        // Credentials are set in inner OkxWs
    }
}
