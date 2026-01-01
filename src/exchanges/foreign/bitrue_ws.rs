//! Bitrue WebSocket Implementation
//!
//! Bitrue WebSocket API for real-time market data
//! Public URL: wss://ws.bitrue.com/market/ws
//! Private URL: wss://wsapi.bitrue.com

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage,
    WsTickerEvent, WsTradeEvent, WsOrderBookEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws.bitrue.com/market/ws";

/// Bitrue WebSocket client
pub struct BitrueWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl BitrueWs {
    /// Create a new Bitrue WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Convert unified symbol to Bitrue format
    /// BTC/USDT -> btcusdt
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Convert Bitrue symbol to unified format
    /// BTCUSDT -> BTC/USDT (for common quote currencies)
    fn parse_symbol(&self, symbol: &str) -> String {
        let symbol_upper = symbol.to_uppercase();
        let quote_currencies = ["USDT", "USDC", "BTC", "ETH", "BUSD", "BNB"];

        for quote in &quote_currencies {
            if symbol_upper.ends_with(quote) && symbol_upper.len() > quote.len() {
                let base = &symbol_upper[..symbol_upper.len() - quote.len()];
                return format!("{}/{}", base, quote);
            }
        }
        symbol_upper
    }

    /// Send a subscription message
    async fn subscribe(&self, channel: &str, cb_id: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "event": "sub",
                "params": {
                    "cb_id": cb_id,
                    "channel": channel,
                },
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string().into()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_PUBLIC_URL.to_string(),
                    message: format!("Failed to send subscribe: {}", e),
                })?;
        }
        Ok(())
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();

        tokio::spawn(async move {
            if let Some(ws) = ws_stream {
                loop {
                    let msg = {
                        let mut ws_guard = ws.write().await;
                        ws_guard.next().await
                    };

                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache, &ws).await;
                            }
                        }
                        Some(Ok(Message::Binary(bin))) => {
                            // Try to parse as UTF-8 text
                            if let Ok(text) = String::from_utf8(bin.to_vec()) {
                                if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                    Self::handle_message_static(&data, &subscriptions, &orderbook_cache, &ws).await;
                                }
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let mut ws_guard = ws.write().await;
                            let _ = ws_guard.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Err(_)) => break,
                        None => break,
                        _ => {}
                    }
                }
            }
        });
    }

    /// Handle incoming WebSocket messages
    async fn handle_message_static(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        ws: &Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
    ) {
        // Handle ping messages
        if let Some(ping) = data.get("ping").and_then(|v| v.as_i64()) {
            let pong_msg = json!({
                "pong": ping,
            });
            let mut ws_guard = ws.write().await;
            let _ = ws_guard.send(Message::Text(pong_msg.to_string().into())).await;
            return;
        }

        // Handle channel messages
        if let Some(channel) = data.get("channel").and_then(|v| v.as_str()) {
            if channel.contains("_simple_depth_") {
                Self::handle_orderbook(data, subscriptions, orderbook_cache).await;
            } else if channel.contains("_trade_") {
                Self::handle_trade(data, subscriptions).await;
            } else if channel.contains("_ticker_") {
                Self::handle_ticker(data, subscriptions).await;
            }
        }
    }

    /// Handle orderbook message
    async fn handle_orderbook(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('_').collect();

        // Extract market id from channel: market_btcusdt_simple_depth_step0
        if parts.len() < 2 {
            return;
        }

        let market_id = parts[1].to_uppercase();
        let symbol = Self::parse_symbol_static(&market_id);
        let key = format!("orderbook:{}", symbol);

        let timestamp = data.get("ts").and_then(|v| v.as_i64());

        if let Some(tick) = data.get("tick") {
            let bids = Self::parse_orderbook_side(tick.get("buys"));
            let asks = Self::parse_orderbook_side(tick.get("asks"));

            let orderbook = OrderBook {
                symbol: symbol.clone(),
                bids,
                asks,
                timestamp,
                datetime: None,
                nonce: None,
            };

            // Update cache
            {
                let mut cache = orderbook_cache.write().await;
                cache.insert(symbol.clone(), orderbook.clone());
            }

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::OrderBook(WsOrderBookEvent {
                    symbol: symbol.clone(),
                    order_book: orderbook,
                    is_snapshot: true,
                }));
            }
        }
    }

    /// Parse orderbook side
    fn parse_orderbook_side(data: Option<&Value>) -> Vec<OrderBookEntry> {
        let mut entries = Vec::new();
        if let Some(arr) = data.and_then(|v| v.as_array()) {
            for item in arr {
                if let Some(entry_arr) = item.as_array() {
                    if entry_arr.len() >= 2 {
                        let price: Decimal = entry_arr[0].as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| entry_arr[0].as_f64().and_then(Decimal::from_f64))
                            .unwrap_or_default();
                        let amount: Decimal = entry_arr[1].as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| entry_arr[1].as_f64().and_then(Decimal::from_f64))
                            .unwrap_or_default();
                        entries.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }
        entries
    }

    /// Handle trade message
    async fn handle_trade(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('_').collect();

        if parts.len() < 2 {
            return;
        }

        let market_id = parts[1].to_uppercase();
        let symbol = Self::parse_symbol_static(&market_id);
        let key = format!("trades:{}", symbol);

        if let Some(tick) = data.get("tick") {
            if let Some(trades) = tick.get("data").and_then(|v| v.as_array()) {
                for trade_data in trades {
                    let timestamp = trade_data.get("ts").and_then(|v| v.as_i64());
                    let side = trade_data.get("side").and_then(|v| v.as_str())
                        .map(|s| if s == "BUY" { "buy" } else { "sell" });

                    let price: Decimal = trade_data.get("price").and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();
                    let amount: Decimal = trade_data.get("vol").and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok()).unwrap_or_default();

                    let trade = Trade::new(
                        trade_data.get("id").and_then(|v| v.as_str())
                            .map(String::from).unwrap_or_default(),
                        symbol.clone(),
                        price,
                        amount,
                    );
                    let trade = if let Some(ts) = timestamp {
                        trade.with_timestamp(ts)
                    } else {
                        trade
                    };
                    let trade = if let Some(s) = side {
                        trade.with_side(s)
                    } else {
                        trade
                    };

                    let subs = subscriptions.read().await;
                    if let Some(sender) = subs.get(&key) {
                        let _ = sender.send(WsMessage::Trade(WsTradeEvent {
                            symbol: symbol.clone(),
                            trades: vec![trade],
                        }));
                    }
                }
            }
        }
    }

    /// Handle ticker message
    async fn handle_ticker(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = channel.split('_').collect();

        if parts.len() < 2 {
            return;
        }

        let market_id = parts[1].to_uppercase();
        let symbol = Self::parse_symbol_static(&market_id);
        let key = format!("ticker:{}", symbol);

        if let Some(tick) = data.get("tick") {
            let timestamp = data.get("ts").and_then(|v| v.as_i64());

            let ticker = Ticker {
                symbol: symbol.clone(),
                high: tick.get("high").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                low: tick.get("low").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                last: tick.get("close").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                bid: tick.get("bid").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                ask: tick.get("ask").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                base_volume: tick.get("vol").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()),
                timestamp,
                ..Default::default()
            };

            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&key) {
                let _ = sender.send(WsMessage::Ticker(WsTickerEvent {
                    symbol: symbol.clone(),
                    ticker,
                }));
            }
        }
    }

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        let symbol_upper = symbol.to_uppercase();
        let quote_currencies = ["USDT", "USDC", "BTC", "ETH", "BUSD", "BNB"];

        for quote in &quote_currencies {
            if symbol_upper.ends_with(quote) && symbol_upper.len() > quote.len() {
                let base = &symbol_upper[..symbol_upper.len() - quote.len()];
                return format!("{}/{}", base, quote);
            }
        }
        symbol_upper
    }
}

impl Default for BitrueWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitrueWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_PUBLIC_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_PUBLIC_URL.to_string(),
                message: format!("WebSocket connection failed: {}", e),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));
        self.start_message_loop();

        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard
                .close(None)
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_PUBLIC_URL.to_string(),
                    message: format!("Failed to close WebSocket: {}", e),
                })?;
        }
        self.ws_stream = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("ticker:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let channel = format!("market_{}_ticker", market_id);
        self.subscribe(&channel, &market_id).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("ticker:{}", symbol);
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            let channel = format!("market_{}_ticker", market_id);
            self.subscribe(&channel, &market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("orderbook:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let channel = format!("market_{}_simple_depth_step0", market_id);
        self.subscribe(&channel, &market_id).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("orderbook:{}", symbol);
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            let channel = format!("market_{}_simple_depth_step0", market_id);
            self.subscribe(&channel, &market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let key = format!("trades:{}", symbol);

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(key, tx);
        }

        let market_id = self.format_symbol(symbol);
        let channel = format!("market_{}_trade_ticker", market_id);
        self.subscribe(&channel, &market_id).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let key = format!("trades:{}", symbol);
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(key, tx.clone());
            }

            let market_id = self.format_symbol(symbol);
            let channel = format!("market_{}_trade_ticker", market_id);
            self.subscribe(&channel, &market_id).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bitrue public WebSocket does not support OHLCV
        Err(CcxtError::NotSupported {
            feature: "Bitrue WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bitrue public WebSocket does not support OHLCV
        Err(CcxtError::NotSupported {
            feature: "Bitrue WebSocket does not support OHLCV".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitrue_ws_creation() {
        let _ws = BitrueWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = BitrueWs::new();
        assert_eq!(ws.format_symbol("BTC/USDT"), "btcusdt");
        assert_eq!(ws.format_symbol("ETH/BTC"), "ethbtc");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = BitrueWs::new();
        assert_eq!(ws.parse_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(ws.parse_symbol("ETHBTC"), "ETH/BTC");
    }
}
