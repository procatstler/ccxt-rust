//! Huobi WebSocket Implementation
//!
//! Alias for HTX WebSocket (same API, previous branding name)

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use crate::types::{Timeframe, WsExchange, WsMessage};

use super::htx_ws::HtxWs;

/// Huobi WebSocket 클라이언트 (HtxWs 래퍼)
///
/// Huobi는 HTX의 이전 브랜드명으로, 동일한 WebSocket API를 사용합니다.
/// HTX는 2023년에 Huobi에서 리브랜딩되었습니다.
pub struct HuobiWs(HtxWs);

impl HuobiWs {
    /// 새 Huobi WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self(HtxWs::new())
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self(HtxWs::with_credentials(api_key, api_secret))
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.0.set_credentials(api_key, api_secret);
    }
}

impl Default for HuobiWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for HuobiWs {
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

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_orders(symbol).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_my_trades(symbol).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.0.watch_balance().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_huobi_ws_creation() {
        let _ws = HuobiWs::new();
    }

    #[test]
    fn test_with_credentials() {
        let _ws = HuobiWs::with_credentials("api_key".to_string(), "api_secret".to_string());
        // Credentials are set in inner HtxWs (private fields)
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = HuobiWs::new();
        ws.set_credentials("api_key".to_string(), "api_secret".to_string());
        // Credentials are set in inner HtxWs (private fields)
    }
}
