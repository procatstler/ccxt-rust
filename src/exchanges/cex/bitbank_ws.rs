//! Bitbank WebSocket Implementation
//!
//! Bitbank WebSocket API for real-time market data
//! Uses socket.io 4.x (Engine.io protocol v4)
//! URL: wss://stream.bitbank.cc/socket.io/?EIO=4&transport=websocket

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
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

const WS_URL: &str = "wss://stream.bitbank.cc/socket.io/?EIO=4&transport=websocket";

/// Bitbank WebSocket client
pub struct BitbankWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl BitbankWs {
    /// Create a new Bitbank WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Convert unified symbol to Bitbank format
    /// BTC/JPY -> btc_jpy
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// Convert Bitbank symbol to unified format
    /// btc_jpy -> BTC/JPY
    fn parse_symbol(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            symbol.to_uppercase()
        }
    }

    /// Send socket.io join-room message
    async fn subscribe(&self, channel: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            // Socket.io event format: 42["join-room","channel_name"]
            let msg = format!(r#"42["join-room","{channel}"]"#);

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send subscribe: {e}"),
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
                            // Handle socket.io protocol messages
                            Self::process_socketio_message(
                                &text,
                                &ws,
                                &subscriptions,
                                &orderbook_cache,
                            )
                            .await;
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let mut ws_guard = ws.write().await;
                            let _ = ws_guard.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) => break,
                        None => break,
                        _ => {}
                    }
                }
            }
        });
    }

    /// Process socket.io protocol message
    async fn process_socketio_message(
        text: &str,
        ws: &Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Socket.io message types:
        // 0 - connect (initial)
        // 2 - ping from server
        // 3 - pong response
        // 40 - connect ack
        // 42 - event (actual data)

        if text == "2" {
            // Server ping, send pong
            let mut ws_guard = ws.write().await;
            let _ = ws_guard.send(Message::Text("3".to_string())).await;
            return;
        }

        if text.starts_with("42") {
            // Event message
            if let Some(json_str) = text.strip_prefix("42") {
                if let Ok(data) = serde_json::from_str::<Value>(json_str) {
                    Self::process_event(&data, subscriptions, orderbook_cache).await;
                }
            }
        }
    }

    /// Process socket.io event
    async fn process_event(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Event format: ["message", {"room_name": "...", "message": {"data": {...}}}]
        if let Some(arr) = data.as_array() {
            if arr.len() >= 2 && arr[0].as_str() == Some("message") {
                if let Some(payload) = arr[1].as_object() {
                    let room_name = payload.get("room_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let message_data = payload.get("message")
                        .and_then(|m| m.get("data"));

                    if let Some(msg_data) = message_data {
                        Self::process_room_message(room_name, msg_data, subscriptions, orderbook_cache).await;
                    }
                }
            }
        }
    }

    /// Process message from a specific room
    async fn process_room_message(
        room_name: &str,
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Parse room name to determine type and pair
        if room_name.starts_with("ticker_") {
            let pair = room_name.strip_prefix("ticker_").unwrap_or("");
            Self::process_ticker(pair, data, subscriptions).await;
        } else if room_name.starts_with("transactions_") {
            let pair = room_name.strip_prefix("transactions_").unwrap_or("");
            Self::process_transactions(pair, data, subscriptions).await;
        } else if room_name.starts_with("depth_whole_") {
            let pair = room_name.strip_prefix("depth_whole_").unwrap_or("");
            Self::process_depth_whole(pair, data, subscriptions, orderbook_cache).await;
        } else if room_name.starts_with("depth_diff_") {
            let pair = room_name.strip_prefix("depth_diff_").unwrap_or("");
            Self::process_depth_diff(pair, data, subscriptions, orderbook_cache).await;
        }
    }

    /// Process ticker message
    async fn process_ticker(
        pair: &str,
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let symbol = Self::pair_to_symbol(pair);

        let timestamp = data.get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            high: data.get("high").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            low: data.get("low").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            bid: data.get("buy").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: None,
            ask: data.get("sell").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.get("last").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            last: data.get("last").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.get("vol").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: data.clone(),
        };

        let sub_key = format!("ticker:{symbol}");
        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&sub_key) {
            let event = WsTickerEvent {
                symbol: symbol.clone(),
                ticker,
            };
            let _ = sender.send(WsMessage::Ticker(event));
        }
    }

    /// Process transactions (trades) message
    async fn process_transactions(
        pair: &str,
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        let symbol = Self::pair_to_symbol(pair);

        let transactions = match data.get("transactions").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return,
        };

        let trades: Vec<Trade> = transactions
            .iter()
            .map(|t| {
                let id = t.get("transaction_id")
                    .and_then(|v| v.as_i64())
                    .map(|i| i.to_string())
                    .unwrap_or_default();

                let timestamp = t.get("executed_at")
                    .and_then(|v| v.as_i64())
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let price = t.get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();

                let amount = t.get("amount")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or_default();

                let side = t.get("side")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase());

                Trade {
                    id,
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side,
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: None,
                    fee: None,
                    fees: Vec::new(),
                    info: t.clone(),
                }
            })
            .collect();

        if !trades.is_empty() {
            let sub_key = format!("trades:{symbol}");
            let subs = subscriptions.read().await;
            if let Some(sender) = subs.get(&sub_key) {
                let event = WsTradeEvent {
                    symbol: symbol.clone(),
                    trades,
                };
                let _ = sender.send(WsMessage::Trade(event));
            }
        }
    }

    /// Process depth_whole (full orderbook) message
    async fn process_depth_whole(
        pair: &str,
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let symbol = Self::pair_to_symbol(pair);

        let bids = Self::parse_orderbook_side(data.get("bids"));
        let asks = Self::parse_orderbook_side(data.get("asks"));

        let timestamp = data.get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

        let orderbook = OrderBook {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            nonce: data.get("sequenceId").and_then(|v| v.as_i64()),
            bids,
            asks,
        };

        // Update cache
        {
            let mut cache = orderbook_cache.write().await;
            cache.insert(symbol.clone(), orderbook.clone());
        }

        // Send snapshot event
        let sub_key = format!("orderbook:{symbol}");
        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&sub_key) {
            let event = WsOrderBookEvent {
                symbol: symbol.clone(),
                order_book: orderbook,
                is_snapshot: true,
            };
            let _ = sender.send(WsMessage::OrderBook(event));
        }
    }

    /// Process depth_diff (orderbook delta) message
    async fn process_depth_diff(
        pair: &str,
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let symbol = Self::pair_to_symbol(pair);

        let bid_updates = Self::parse_orderbook_side(data.get("b"));
        let ask_updates = Self::parse_orderbook_side(data.get("a"));

        let mut cache = orderbook_cache.write().await;
        let orderbook = cache.entry(symbol.clone()).or_insert_with(|| OrderBook {
            symbol: symbol.clone(),
            timestamp: None,
            datetime: None,
            nonce: None,
            bids: Vec::new(),
            asks: Vec::new(),
        });

        // Apply bid updates
        for update in bid_updates {
            if update.amount.is_zero() {
                orderbook.bids.retain(|e| e.price != update.price);
            } else if let Some(existing) = orderbook.bids.iter_mut().find(|e| e.price == update.price) {
                existing.amount = update.amount;
            } else {
                orderbook.bids.push(update);
            }
        }
        orderbook.bids.sort_by(|a, b| b.price.cmp(&a.price));

        // Apply ask updates
        for update in ask_updates {
            if update.amount.is_zero() {
                orderbook.asks.retain(|e| e.price != update.price);
            } else if let Some(existing) = orderbook.asks.iter_mut().find(|e| e.price == update.price) {
                existing.amount = update.amount;
            } else {
                orderbook.asks.push(update);
            }
        }
        orderbook.asks.sort_by(|a, b| a.price.cmp(&b.price));

        let timestamp = data.get("t")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        orderbook.timestamp = Some(timestamp);
        orderbook.datetime = Some(chrono::DateTime::from_timestamp_millis(timestamp)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default());
        orderbook.nonce = data.get("s").and_then(|v| v.as_i64());

        let updated_orderbook = orderbook.clone();
        drop(cache);

        // Send update event
        let sub_key = format!("orderbook:{symbol}");
        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&sub_key) {
            let event = WsOrderBookEvent {
                symbol: symbol.clone(),
                order_book: updated_orderbook,
                is_snapshot: false,
            };
            let _ = sender.send(WsMessage::OrderBook(event));
        }
    }

    /// Parse orderbook side from JSON
    fn parse_orderbook_side(data: Option<&Value>) -> Vec<OrderBookEntry> {
        let arr = match data {
            Some(Value::Array(a)) => a,
            _ => return Vec::new(),
        };

        arr.iter()
            .filter_map(|item| {
                // Format: ["price", "amount"] as strings
                if let Some(arr) = item.as_array() {
                    if arr.len() >= 2 {
                        let price = arr[0].as_str()
                            .and_then(|s| Decimal::from_str(s).ok())?;
                        let amount = arr[1].as_str()
                            .and_then(|s| Decimal::from_str(s).ok())?;
                        return Some(OrderBookEntry { price, amount });
                    }
                }
                None
            })
            .collect()
    }

    /// Convert pair to unified symbol
    fn pair_to_symbol(pair: &str) -> String {
        let parts: Vec<&str> = pair.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            pair.to_uppercase()
        }
    }
}

impl Default for BitbankWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitbankWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        let (ws_stream, _) = connect_async(WS_URL)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("Failed to connect: {e}"),
            })?;

        self.ws_stream = Some(Arc::new(RwLock::new(ws_stream)));
        self.start_message_loop();
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let mut ws_guard = ws.write().await;
            ws_guard.close(None).await.ok();
        }
        self.ws_stream = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_stream.is_some()
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sub_key = format!("ticker:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(sub_key.clone(), tx);
        }

        let pair = self.format_symbol(symbol);
        let channel = format!("ticker_{pair}");
        self.subscribe(&channel).await?;

        Ok(rx)
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let sub_key = format!("ticker:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(sub_key.clone(), tx.clone());
            }

            let pair = self.format_symbol(symbol);
            let channel = format!("ticker_{pair}");
            self.subscribe(&channel).await?;
        }

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sub_key = format!("trades:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(sub_key.clone(), tx);
        }

        let pair = self.format_symbol(symbol);
        let channel = format!("transactions_{pair}");
        self.subscribe(&channel).await?;

        Ok(rx)
    }

    async fn watch_trades_for_symbols(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let sub_key = format!("trades:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(sub_key.clone(), tx.clone());
            }

            let pair = self.format_symbol(symbol);
            let channel = format!("transactions_{pair}");
            self.subscribe(&channel).await?;
        }

        Ok(rx)
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let sub_key = format!("orderbook:{symbol}");

        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(sub_key.clone(), tx);
        }

        let pair = self.format_symbol(symbol);

        // Subscribe to both whole and diff channels
        let whole_channel = format!("depth_whole_{pair}");
        let diff_channel = format!("depth_diff_{pair}");

        self.subscribe(&whole_channel).await?;
        self.subscribe(&diff_channel).await?;

        Ok(rx)
    }

    async fn watch_order_book_for_symbols(&self, symbols: &[&str], _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();

        for symbol in symbols {
            let sub_key = format!("orderbook:{symbol}");
            {
                let mut subs = self.subscriptions.write().await;
                subs.insert(sub_key.clone(), tx.clone());
            }

            let pair = self.format_symbol(symbol);

            let whole_channel = format!("depth_whole_{pair}");
            let diff_channel = format!("depth_diff_{pair}");

            self.subscribe(&whole_channel).await?;
            self.subscribe(&diff_channel).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bitbank does not support OHLCV via WebSocket
        Err(CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
        })
    }

    async fn watch_ohlcv_for_symbols(&self, _symbols: &[&str], _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_ohlcv_for_symbols".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        let ws = BitbankWs::new();
        assert_eq!(ws.format_symbol("BTC/JPY"), "btc_jpy");
        assert_eq!(ws.format_symbol("ETH/JPY"), "eth_jpy");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = BitbankWs::new();
        assert_eq!(ws.parse_symbol("btc_jpy"), "BTC/JPY");
        assert_eq!(ws.parse_symbol("eth_jpy"), "ETH/JPY");
    }

    #[test]
    fn test_pair_to_symbol() {
        assert_eq!(BitbankWs::pair_to_symbol("btc_jpy"), "BTC/JPY");
        assert_eq!(BitbankWs::pair_to_symbol("eth_usdt"), "ETH/USDT");
    }

    #[tokio::test]
    async fn test_default() {
        let ws = BitbankWs::default();
        assert!(!ws.ws_is_connected().await);
    }
}
