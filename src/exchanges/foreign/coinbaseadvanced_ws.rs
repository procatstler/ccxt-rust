//! Coinbase Advanced WebSocket Implementation
//!
//! Alias for Coinbase WebSocket (Advanced Trade API)

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use crate::types::{Timeframe, WsExchange, WsMessage};

use super::coinbase_ws::CoinbaseWs;

/// Coinbase Advanced WebSocket 클라이언트 (CoinbaseWs 래퍼)
///
/// Coinbase Advanced는 Coinbase의 Advanced Trade API를 사용합니다.
/// - WebSocket URL: wss://advanced-trade-ws.coinbase.com
///
/// Note: CoinbaseWs가 이미 Advanced Trade API를 사용하므로 이 클래스는
/// 단순히 CoinbaseWs를 래핑하여 ccxt의 명명 규칙을 따릅니다.
pub struct CoinbaseAdvancedWs(CoinbaseWs);

impl CoinbaseAdvancedWs {
    /// 새 Coinbase Advanced WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self(CoinbaseWs::new())
    }
}

impl Default for CoinbaseAdvancedWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinbaseAdvancedWs {
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
    fn test_coinbaseadvanced_ws_creation() {
        let _ws = CoinbaseAdvancedWs::new();
    }
}
