//! bitFlyer WebSocket Implementation
//!
//! bitFlyer Lightning WebSocket API for real-time market data
//! Uses JSON-RPC 2.0 over WebSocket
//! URL: wss://ws.lightstream.bitflyer.com/json-rpc

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

const WS_URL: &str = "wss://ws.lightstream.bitflyer.com/json-rpc";

/// bitFlyer WebSocket client
pub struct BitflyerWs {
    ws_stream: Option<Arc<RwLock<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl BitflyerWs {
    /// Create a new bitFlyer WebSocket client
    pub fn new() -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: None,
            api_secret: None,
        }
    }

    /// Create a new bitFlyer WebSocket client with credentials
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_stream: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Convert unified symbol to bitFlyer format
    /// BTC/JPY -> BTC_JPY
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Convert bitFlyer symbol to unified format
    /// BTC_JPY -> BTC/JPY
    fn parse_symbol(&self, symbol: &str) -> String {
        symbol.replace("_", "/")
    }

    /// Send a JSON-RPC subscription message
    async fn subscribe(&self, channel: &str) -> CcxtResult<()> {
        if let Some(ws) = &self.ws_stream {
            let msg = json!({
                "method": "subscribe",
                "params": {
                    "channel": channel
                },
                "id": null
            });

            let mut ws_guard = ws.write().await;
            ws_guard
                .send(Message::Text(msg.to_string()))
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
                            if let Ok(data) = serde_json::from_str::<Value>(&text) {
                                Self::process_message(
                                    &data,
                                    &subscriptions,
                                    &orderbook_cache,
                                )
                                .await;
                            }
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

    /// Process incoming WebSocket message
    async fn process_message(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // JSON-RPC 2.0 channelMessage format
        // {"method": "channelMessage", "params": {"channel": "...", "message": {...}}}
        let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("");

        if method != "channelMessage" {
            return;
        }

        let params = match data.get("params") {
            Some(p) => p,
            None => return,
        };

        let channel = match params.get("channel").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return,
        };

        let message = match params.get("message") {
            Some(m) => m,
            None => return,
        };

        // Parse channel name to determine type and symbol
        // Channels: lightning_ticker_BTC_JPY, lightning_executions_BTC_JPY,
        //           lightning_board_snapshot_BTC_JPY, lightning_board_BTC_JPY
        if channel.starts_with("lightning_ticker_") {
            let product_code = channel.strip_prefix("lightning_ticker_").unwrap_or("");
            let symbol = product_code.replace("_", "/");
            Self::process_ticker(&symbol, message, subscriptions).await;
        } else if channel.starts_with("lightning_executions_") {
            let product_code = channel.strip_prefix("lightning_executions_").unwrap_or("");
            let symbol = product_code.replace("_", "/");
            Self::process_executions(&symbol, message, subscriptions).await;
        } else if channel.starts_with("lightning_board_snapshot_") {
            let product_code = channel.strip_prefix("lightning_board_snapshot_").unwrap_or("");
            let symbol = product_code.replace("_", "/");
            Self::process_board_snapshot(&symbol, message, subscriptions, orderbook_cache).await;
        } else if channel.starts_with("lightning_board_") && !channel.contains("snapshot") {
            let product_code = channel.strip_prefix("lightning_board_").unwrap_or("");
            let symbol = product_code.replace("_", "/");
            Self::process_board_update(&symbol, message, subscriptions, orderbook_cache).await;
        }
    }

    /// Process ticker message
    async fn process_ticker(
        symbol: &str,
        message: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        // Ticker format:
        // {"product_code": "BTC_JPY", "timestamp": "2021-01-01T00:00:00.000",
        //  "best_bid": 1234567, "best_ask": 1234568, "best_bid_size": 0.1, "best_ask_size": 0.2,
        //  "total_bid_depth": 100, "total_ask_depth": 100, "ltp": 1234567.5,
        //  "volume": 1000, "volume_by_product": 500}
        let timestamp = message.get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: message.get("timestamp").and_then(|v| v.as_str()).map(|s| s.to_string()),
            high: None,
            low: None,
            bid: message.get("best_bid").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            bid_volume: message.get("best_bid_size").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            ask: message.get("best_ask").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            ask_volume: message.get("best_ask_size").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            vwap: None,
            open: None,
            close: message.get("ltp").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            last: message.get("ltp").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: message.get("volume").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            quote_volume: message.get("volume_by_product").and_then(|v| v.as_f64()).map(|f| Decimal::from_f64(f).unwrap_or_default()),
            index_price: None,
            mark_price: None,
            info: message.clone(),
        };

        let sub_key = format!("ticker:{symbol}");
        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&sub_key) {
            let event = WsTickerEvent {
                symbol: symbol.to_string(),
                ticker,
            };
            let _ = sender.send(WsMessage::Ticker(event));
        }
    }

    /// Process executions (trades) message
    async fn process_executions(
        symbol: &str,
        message: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        // Executions format (array of trades):
        // [{"id": 123456, "side": "BUY", "price": 1234567, "size": 0.1, "exec_date": "2021-01-01T00:00:00.000"}, ...]
        let trades_data = if message.is_array() {
            message.as_array().unwrap()
        } else {
            return;
        };

        let trades: Vec<Trade> = trades_data
            .iter()
            .map(|t| {
                let id = t.get("id")
                    .and_then(|v| v.as_i64())
                    .map(|i| i.to_string())
                    .unwrap_or_default();

                let timestamp = t.get("exec_date")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                let price = t.get("price")
                    .and_then(|v| v.as_f64())
                    .map(|f| Decimal::from_f64(f).unwrap_or_default())
                    .unwrap_or_default();

                let amount = t.get("size")
                    .and_then(|v| v.as_f64())
                    .map(|f| Decimal::from_f64(f).unwrap_or_default())
                    .unwrap_or_default();

                let side = t.get("side")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_lowercase());

                Trade {
                    id,
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: t.get("exec_date").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    symbol: symbol.to_string(),
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
                    symbol: symbol.to_string(),
                    trades,
                };
                let _ = sender.send(WsMessage::Trade(event));
            }
        }
    }

    /// Process board snapshot message
    async fn process_board_snapshot(
        symbol: &str,
        message: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Board snapshot format:
        // {"mid_price": 1234567, "bids": [{"price": 1234566, "size": 0.1}, ...], "asks": [{"price": 1234568, "size": 0.2}, ...]}
        let bids = Self::parse_orderbook_side(message.get("bids"));
        let asks = Self::parse_orderbook_side(message.get("asks"));

        let timestamp = chrono::Utc::now().timestamp_millis();

        let orderbook = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        };

        // Update cache
        {
            let mut cache = orderbook_cache.write().await;
            cache.insert(symbol.to_string(), orderbook.clone());
        }

        // Send snapshot event
        let sub_key = format!("orderbook:{symbol}");
        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&sub_key) {
            let event = WsOrderBookEvent {
                symbol: symbol.to_string(),
                order_book: orderbook,
                is_snapshot: true,
            };
            let _ = sender.send(WsMessage::OrderBook(event));
        }
    }

    /// Process board update (delta) message
    async fn process_board_update(
        symbol: &str,
        message: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        // Board update format:
        // {"mid_price": 1234567, "bids": [{"price": 1234566, "size": 0.1}, ...], "asks": [{"price": 1234568, "size": 0.2}, ...]}
        // size = 0 means the price level should be removed
        let bid_updates = Self::parse_orderbook_side(message.get("bids"));
        let ask_updates = Self::parse_orderbook_side(message.get("asks"));

        let mut cache = orderbook_cache.write().await;
        let orderbook = cache.entry(symbol.to_string()).or_insert_with(|| OrderBook {
            symbol: symbol.to_string(),
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
        // Sort bids descending by price
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
        // Sort asks ascending by price
        orderbook.asks.sort_by(|a, b| a.price.cmp(&b.price));

        let timestamp = chrono::Utc::now().timestamp_millis();
        orderbook.timestamp = Some(timestamp);
        orderbook.datetime = Some(chrono::Utc::now().to_rfc3339());

        let updated_orderbook = orderbook.clone();
        drop(cache);

        // Send update event
        let sub_key = format!("orderbook:{symbol}");
        let subs = subscriptions.read().await;
        if let Some(sender) = subs.get(&sub_key) {
            let event = WsOrderBookEvent {
                symbol: symbol.to_string(),
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
                let price = item.get("price")
                    .and_then(|v| v.as_f64())
                    .map(|f| Decimal::from_f64(f).unwrap_or_default())?;
                let amount = item.get("size")
                    .and_then(|v| v.as_f64())
                    .map(|f| Decimal::from_f64(f).unwrap_or_default())?;
                Some(OrderBookEntry { price, amount })
            })
            .collect()
    }
}

impl Default for BitflyerWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitflyerWs {
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

        let product_code = self.format_symbol(symbol);
        let channel = format!("lightning_ticker_{product_code}");
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

            let product_code = self.format_symbol(symbol);
            let channel = format!("lightning_ticker_{product_code}");
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

        let product_code = self.format_symbol(symbol);
        let channel = format!("lightning_executions_{product_code}");
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

            let product_code = self.format_symbol(symbol);
            let channel = format!("lightning_executions_{product_code}");
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

        let product_code = self.format_symbol(symbol);

        // Subscribe to both snapshot and updates
        let snapshot_channel = format!("lightning_board_snapshot_{product_code}");
        let update_channel = format!("lightning_board_{product_code}");

        self.subscribe(&snapshot_channel).await?;
        self.subscribe(&update_channel).await?;

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

            let product_code = self.format_symbol(symbol);

            let snapshot_channel = format!("lightning_board_snapshot_{product_code}");
            let update_channel = format!("lightning_board_{product_code}");

            self.subscribe(&snapshot_channel).await?;
            self.subscribe(&update_channel).await?;
        }

        Ok(rx)
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // bitFlyer does not support OHLCV via WebSocket
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
        let ws = BitflyerWs::new();
        assert_eq!(ws.format_symbol("BTC/JPY"), "BTC_JPY");
        assert_eq!(ws.format_symbol("ETH/JPY"), "ETH_JPY");
    }

    #[test]
    fn test_parse_symbol() {
        let ws = BitflyerWs::new();
        assert_eq!(ws.parse_symbol("BTC_JPY"), "BTC/JPY");
        assert_eq!(ws.parse_symbol("ETH_JPY"), "ETH/JPY");
    }

    #[tokio::test]
    async fn test_default() {
        let ws = BitflyerWs::default();
        assert!(!ws.ws_is_connected().await);
    }
}
