//! Binance US WebSocket Implementation
//!
//! Alias for Binance WebSocket (US region variant)

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use crate::types::{Timeframe, WsExchange, WsMessage};

use super::binance_ws::BinanceWs;

/// Binance US WebSocket 클라이언트 (BinanceWs 래퍼)
///
/// Binance US는 Binance의 미국 지역 버전으로, 동일한 WebSocket API를 사용합니다.
/// - WebSocket URL: wss://stream.binance.us:9443/ws
pub struct BinanceUsWs(BinanceWs);

impl BinanceUsWs {
    /// 새 Binance US WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self(BinanceWs::new())
    }
}

impl Default for BinanceUsWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BinanceUsWs {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binanceus_ws_creation() {
        let _ws = BinanceUsWs::new();
    }
}
