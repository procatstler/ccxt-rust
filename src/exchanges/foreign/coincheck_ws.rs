//! Coincheck WebSocket Implementation
//!
//! Coincheck WebSocket API for real-time market data
//! URL: wss://ws-api.coincheck.com/

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

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Timeframe, Trade, WsExchange, WsMessage,
    WsTradeEvent, WsOrderBookEvent,
};

const WS_URL: &str = "wss://ws-api.coincheck.com/";

/// Coincheck WebSocket client
pub struct CoincheckWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl CoincheckWs {
    /// Create a new Coincheck WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Convert unified symbol to Coincheck format
    /// BTC/JPY -> btc_jpy
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// Convert Coincheck symbol to unified format
    /// btc_jpy -> BTC/JPY
    fn parse_symbol(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }

    /// Send a subscription message
    async fn subscribe(&self, market_id: &str, channel_type: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let channel = format!("{}-{}", market_id, channel_type);
            let msg = json!({
                "type": "subscribe",
                "channel": channel,
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string().into()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
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
                                Self::handle_message_static(&data, &subscriptions, &orderbook_cache).await;
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
    ) {
        // Coincheck messages are arrays: [market_id, data] or [[trade_data, ...]]
        if let Some(arr) = data.as_array() {
            if arr.len() >= 2 {
                // Check if first element is market_id (string) -> orderbook
                if let Some(market_id) = arr[0].as_str() {
                    if let Some(orderbook_data) = arr[1].as_object() {
                        Self::handle_orderbook(market_id, orderbook_data, subscriptions, orderbook_cache).await;
                    }
                }
            } else if arr.len() == 1 {
                // Nested array -> trades
                if let Some(trade_array) = arr[0].as_array() {
                    Self::handle_trades(trade_array, subscriptions).await;
                }
            }
        }
    }

    /// Handle orderbook message
    /// Message format: ["btc_jpy", {"bids": [["price", "amount"]], "asks": [...], "last_update_at": "timestamp"}]
    async fn handle_orderbook(
        market_id: &str,
        orderbook_data: &serde_json::Map<String, Value>,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let symbol = Self::parse_symbol_static(market_id);
        let key = format!("orderbook:{}", symbol);

        let mut bids: Vec<OrderBookEntry> = Vec::new();
        let mut asks: Vec<OrderBookEntry> = Vec::new();

        // Parse bids
        if let Some(bids_array) = orderbook_data.get("bids").and_then(|v| v.as_array()) {
            for bid in bids_array {
                if let Some(bid_arr) = bid.as_array() {
                    if bid_arr.len() >= 2 {
                        let price: Decimal = bid_arr[0].as_str()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default();
                        let amount: Decimal = bid_arr[1].as_str()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default();

                        bids.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        // Parse asks
        if let Some(asks_array) = orderbook_data.get("asks").and_then(|v| v.as_array()) {
            for ask in asks_array {
                if let Some(ask_arr) = ask.as_array() {
                    if ask_arr.len() >= 2 {
                        let price: Decimal = ask_arr[0].as_str()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default();
                        let amount: Decimal = ask_arr[1].as_str()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default();

                        asks.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        // Sort bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        // Get timestamp
        let timestamp = orderbook_data.get("last_update_at")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .map(|ts| ts * 1000); // Convert seconds to milliseconds

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

    /// Handle trades message
    /// Message format: [["timestamp", "trade_id", "pair", "price", "amount", "side", "taker_id", "maker_id"]]
    async fn handle_trades(
        trade_array: &Vec<Value>,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        for trade_data in trade_array {
            if let Some(trade_arr) = trade_data.as_array() {
                if trade_arr.len() >= 6 {
                    // Extract market_id from trade data
                    let market_id = trade_arr[2].as_str().unwrap_or("");
                    let symbol = Self::parse_symbol_static(market_id);
                    let key = format!("trades:{}", symbol);

                    // Parse timestamp (in seconds)
                    let timestamp = trade_arr[0].as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(|ts| ts * 1000); // Convert seconds to milliseconds

                    // Parse trade_id
                    let trade_id = trade_arr[1].as_str()
                        .map(String::from)
                        .unwrap_or_default();

                    // Parse price and amount
                    let price: Decimal = trade_arr[3].as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default();
                    let amount: Decimal = trade_arr[4].as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default();

                    // Parse side
                    let side = trade_arr[5].as_str().unwrap_or("");

                    let mut trade = Trade::new(
                        trade_id,
                        symbol.clone(),
                        price,
                        amount,
                    );

                    if let Some(ts) = timestamp {
                        trade = trade.with_timestamp(ts);
                    }

                    trade = trade.with_side(side);

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

    /// Static helper for parsing symbol
    fn parse_symbol_static(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }
}

impl Default for CoincheckWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoincheckWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
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
                    url: WS_URL.to_string(),
                    message: format!("Failed to close WebSocket: {}", e),
                })?;
        }
        self.ws_stream = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn watch_ticker(&self, _symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coincheck does not support ticker via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Coincheck WebSocket does not support ticker".to_string(),
        })
    }

    async fn watch_tickers(&self, _symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coincheck does not support ticker via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Coincheck WebSocket does not support ticker".to_string(),
        })
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
        self.subscribe(&market_id, "orderbook").await?;

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
            self.subscribe(&market_id, "orderbook").await?;
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
        self.subscribe(&market_id, "trades").await?;

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
            self.subscribe(&market_id, "trades").await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coincheck does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Coincheck WebSocket does not support OHLCV".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coincheck does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "Coincheck WebSocket does not support OHLCV".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coincheck_ws_creation() {
        let _ws = CoincheckWs::new();
    }

    #[test]
    fn test_format_symbol() {
        let ws = CoincheckWs::new();
        assert_eq!(ws.format_symbol("BTC/JPY"), "btc_jpy");
        assert_eq!(ws.format_symbol("ETH/JPY"), "eth_jpy");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = CoincheckWs::new();
        assert_eq!(ws.parse_symbol("btc_jpy"), "BTC/JPY");
        assert_eq!(ws.parse_symbol("eth_jpy"), "ETH/JPY");
    }
}
