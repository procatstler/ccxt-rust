//! Bequant WebSocket Implementation
//!
//! Alias for HitBTC WebSocket with different hostname (api.bequant.io)

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::errors::CcxtResult;
use crate::types::{Timeframe, WsExchange, WsMessage};

use super::hitbtc_ws::HitbtcWs;

/// Bequant WebSocket 클라이언트 (HitbtcWs 래퍼)
///
/// Bequant는 HitBTC와 동일한 WebSocket API를 사용하지만 다른 hostname을 사용합니다.
/// - WebSocket URL: wss://api.bequant.io/api/3/ws/public
pub struct BequantWs(HitbtcWs);

impl BequantWs {
    /// 새 Bequant WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self(HitbtcWs::new())
    }
}

impl Default for BequantWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BequantWs {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bequant_ws_creation() {
        let _ws = BequantWs::new();
    }
}
