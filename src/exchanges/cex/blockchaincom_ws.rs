//! Blockchain.com WebSocket Implementation
//!
//! Blockchain.com WebSocket API for real-time market data
//! URL: wss://ws.blockchain.info/mercury-gateway/v1/ws

#![allow(dead_code)]

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{http::Request as WsRequest, protocol::Message},
    MaybeTlsStream, WebSocketStream,
};

use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_URL: &str = "wss://ws.blockchain.info/mercury-gateway/v1/ws";
const ORIGIN_HEADER: &str = "https://exchange.blockchain.com";

/// Blockchain.com WebSocket client
pub struct BlockchainComWs {
    ws_stream: Arc<RwLock<Option<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    subscriptions: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    orderbook_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    ticker_cache: Arc<RwLock<HashMap<String, Ticker>>>,
    ohlcv_cache: Arc<RwLock<HashMap<String, Vec<OHLCV>>>>,
    ohlcv_timeframes: Arc<RwLock<HashMap<String, Timeframe>>>,
    api_secret: Option<String>,
    authenticated: Arc<RwLock<bool>>,
    timeframes: HashMap<Timeframe, String>,
}

impl BlockchainComWs {
    /// Create a new Blockchain.com WebSocket client
    pub fn new() -> Self {
        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".to_string());
        timeframes.insert(Timeframe::Minute5, "300".to_string());
        timeframes.insert(Timeframe::Minute15, "900".to_string());
        timeframes.insert(Timeframe::Hour1, "3600".to_string());
        timeframes.insert(Timeframe::Hour6, "21600".to_string());
        timeframes.insert(Timeframe::Day1, "86400".to_string());

        Self {
            ws_stream: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            ticker_cache: Arc::new(RwLock::new(HashMap::new())),
            ohlcv_cache: Arc::new(RwLock::new(HashMap::new())),
            ohlcv_timeframes: Arc::new(RwLock::new(HashMap::new())),
            api_secret: None,
            authenticated: Arc::new(RwLock::new(false)),
            timeframes,
        }
    }

    /// Create a new Blockchain.com WebSocket client with credentials
    pub fn with_credentials(api_secret: String) -> Self {
        let mut ws = Self::new();
        ws.api_secret = Some(api_secret);
        ws
    }

    /// Convert unified symbol to Blockchain.com format
    /// BTC/USD -> BTC-USD
    fn format_symbol(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert Blockchain.com symbol to unified format
    /// BTC-USD -> BTC/USD
    fn parse_symbol(&self, symbol: &str) -> String {
        symbol.replace("-", "/")
    }

    /// Convert timeframe to granularity seconds
    fn timeframe_to_granularity(&self, timeframe: Timeframe) -> Option<u64> {
        self.timeframes.get(&timeframe).and_then(|s| s.parse().ok())
    }

    /// Internal close helper for use within &self methods
    async fn internal_close(&self) -> CcxtResult<()> {
        let mut ws_guard = self.ws_stream.write().await;
        if let Some(ref mut ws) = *ws_guard {
            ws.close(None).await.map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("Failed to close WebSocket: {e}"),
            })?;
        }
        *ws_guard = None;

        // Clear subscriptions
        let mut subs = self.subscriptions.write().await;
        subs.clear();

        // Reset authenticated state
        let mut auth = self.authenticated.write().await;
        *auth = false;

        Ok(())
    }

    /// Send a subscription message
    async fn subscribe(
        &self,
        channel: &str,
        market_id: Option<&str>,
        extra_params: Option<Value>,
    ) -> CcxtResult<()> {
        let ws_guard = self.ws_stream.read().await;
        if let Some(ref _ws) = *ws_guard {
            let mut msg = json!({
                "action": "subscribe",
                "channel": channel,
            });

            if let Some(id) = market_id {
                msg["symbol"] = json!(id);
            }

            if let Some(Value::Object(obj)) = extra_params {
                for (key, value) in obj {
                    msg[key] = value;
                }
            }

            drop(ws_guard);
            let mut ws_write = self.ws_stream.write().await;
            if let Some(ref mut ws) = *ws_write {
                ws.send(Message::Text(msg.to_string())).await.map_err(|e| {
                    CcxtError::NetworkError {
                        url: WS_URL.to_string(),
                        message: format!("Failed to send subscribe: {e}"),
                    }
                })?;
            }
        }
        Ok(())
    }

    /// Authenticate with the WebSocket
    async fn authenticate(&self) -> CcxtResult<()> {
        let already_auth = *self.authenticated.read().await;
        if already_auth {
            return Ok(());
        }

        let secret = self
            .api_secret
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required for authentication".into(),
            })?;

        let msg = json!({
            "action": "subscribe",
            "channel": "auth",
            "token": secret,
        });

        let mut ws_guard = self.ws_stream.write().await;
        if let Some(ref mut ws) = *ws_guard {
            ws.send(Message::Text(msg.to_string()))
                .await
                .map_err(|e| CcxtError::NetworkError {
                    url: WS_URL.to_string(),
                    message: format!("Failed to send auth: {e}"),
                })?;
        }

        Ok(())
    }

    /// Start message processing loop
    fn start_message_loop(&self) {
        let ws_stream = self.ws_stream.clone();
        let subscriptions = self.subscriptions.clone();
        let orderbook_cache = self.orderbook_cache.clone();
        let ticker_cache = self.ticker_cache.clone();
        let ohlcv_cache = self.ohlcv_cache.clone();
        let ohlcv_timeframes = self.ohlcv_timeframes.clone();
        let authenticated = self.authenticated.clone();

        tokio::spawn(async move {
            loop {
                let msg = {
                    let mut ws_guard = ws_stream.write().await;
                    if let Some(ref mut ws) = *ws_guard {
                        ws.next().await
                    } else {
                        break;
                    }
                };

                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(data) = serde_json::from_str::<Value>(&text) {
                            Self::handle_message_static(
                                &data,
                                &subscriptions,
                                &orderbook_cache,
                                &ticker_cache,
                                &ohlcv_cache,
                                &ohlcv_timeframes,
                                &authenticated,
                            )
                            .await;
                        }
                    },
                    Some(Ok(Message::Ping(data))) => {
                        let mut ws_guard = ws_stream.write().await;
                        if let Some(ref mut ws) = *ws_guard {
                            let _ = ws.send(Message::Pong(data)).await;
                        }
                    },
                    Some(Ok(Message::Close(_))) => break,
                    Some(Err(_)) => break,
                    None => break,
                    _ => {},
                }
            }
        });
    }

    /// Handle incoming WebSocket messages
    async fn handle_message_static(
        data: &Value,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
        ticker_cache: &Arc<RwLock<HashMap<String, Ticker>>>,
        ohlcv_cache: &Arc<RwLock<HashMap<String, Vec<OHLCV>>>>,
        ohlcv_timeframes: &Arc<RwLock<HashMap<String, Timeframe>>>,
        authenticated: &Arc<RwLock<bool>>,
    ) {
        let channel = data.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let event = data.get("event").and_then(|v| v.as_str()).unwrap_or("");

        // Handle authentication response
        if channel == "auth" {
            if event == "subscribed" {
                let mut auth = authenticated.write().await;
                *auth = true;
            }
            return;
        }

        // Skip subscription confirmations
        if event == "subscribed" {
            return;
        }

        match channel {
            "ticker" => {
                Self::handle_ticker(data, event, subscriptions, ticker_cache).await;
            },
            "trades" => {
                Self::handle_trades(data, event, subscriptions).await;
            },
            "l2" | "l3" => {
                Self::handle_orderbook(data, event, channel, subscriptions, orderbook_cache).await;
            },
            "prices" => {
                Self::handle_ohlcv(data, event, subscriptions, ohlcv_cache, ohlcv_timeframes).await;
            },
            "balances" => {
                // Balance updates - would need Balance type support
            },
            "trading" => {
                // Order updates - would need Order type support
            },
            _ => {},
        }
    }

    /// Handle ticker messages
    async fn handle_ticker(
        data: &Value,
        event: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        ticker_cache: &Arc<RwLock<HashMap<String, Ticker>>>,
    ) {
        let market_id = data.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = market_id.replace("-", "/");
        let message_hash = format!("ticker:{symbol}");

        let ticker = if event == "snapshot" {
            // Full ticker snapshot
            Ticker {
                symbol: symbol.clone(),
                timestamp: None,
                datetime: None,
                high: None,
                low: None,
                bid: None,
                bid_volume: None,
                ask: None,
                ask_volume: None,
                vwap: None,
                open: data
                    .get("price_24h")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::try_from(f).ok()),
                close: None,
                last: data
                    .get("last_trade_price")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::try_from(f).ok()),
                previous_close: None,
                change: None,
                percentage: None,
                average: None,
                base_volume: data
                    .get("volume_24h")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::try_from(f).ok()),
                quote_volume: None,
                index_price: None,
                mark_price: data
                    .get("mark_price")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::try_from(f).ok()),
                info: data.clone(),
            }
        } else if event == "updated" {
            // Incremental update - merge with cached ticker
            let cache = ticker_cache.read().await;
            let prev = cache.get(&symbol);

            Ticker {
                symbol: symbol.clone(),
                timestamp: None,
                datetime: None,
                high: None,
                low: None,
                bid: None,
                bid_volume: None,
                ask: None,
                ask_volume: None,
                vwap: None,
                open: prev.and_then(|t| t.open),
                close: None,
                last: data
                    .get("mark_price")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::try_from(f).ok()),
                previous_close: prev.and_then(|t| t.last),
                change: None,
                percentage: None,
                average: None,
                base_volume: prev.and_then(|t| t.base_volume),
                quote_volume: None,
                index_price: None,
                mark_price: data
                    .get("mark_price")
                    .and_then(|v| v.as_f64())
                    .and_then(|f| Decimal::try_from(f).ok()),
                info: data.clone(),
            }
        } else {
            return;
        };

        // Update cache
        {
            let mut cache = ticker_cache.write().await;
            cache.insert(symbol.clone(), ticker.clone());
        }

        // Send to subscribers
        let subs = subscriptions.read().await;
        if let Some(tx) = subs.get(&message_hash) {
            let event = WsTickerEvent {
                symbol: symbol.clone(),
                ticker,
            };
            let _ = tx.send(WsMessage::Ticker(event));
        }
    }

    /// Handle trade messages
    async fn handle_trades(
        data: &Value,
        event: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    ) {
        if event != "updated" {
            return;
        }

        let market_id = data.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = market_id.replace("-", "/");
        let message_hash = format!("trades:{symbol}");

        let timestamp_str = data.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp_str)
            .map(|dt| dt.timestamp_millis())
            .ok();

        let trade = Trade {
            id: data
                .get("trade_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            order: None,
            timestamp,
            datetime: Some(timestamp_str.to_string()),
            symbol: symbol.clone(),
            trade_type: None,
            side: data
                .get("side")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            taker_or_maker: None,
            price: data
                .get("price")
                .and_then(|v| v.as_f64())
                .and_then(|f| Decimal::try_from(f).ok())
                .unwrap_or_default(),
            amount: data
                .get("qty")
                .and_then(|v| v.as_f64())
                .and_then(|f| Decimal::try_from(f).ok())
                .unwrap_or_default(),
            cost: None,
            fee: None,
            fees: Vec::new(),
            info: data.clone(),
        };

        let subs = subscriptions.read().await;
        if let Some(tx) = subs.get(&message_hash) {
            let event = WsTradeEvent {
                symbol: symbol.clone(),
                trades: vec![trade],
            };
            let _ = tx.send(WsMessage::Trade(event));
        }
    }

    /// Handle orderbook messages
    async fn handle_orderbook(
        data: &Value,
        event: &str,
        _channel: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        orderbook_cache: &Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        let market_id = data.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = market_id.replace("-", "/");
        let message_hash = format!("orderbook:{symbol}");

        let timestamp_str = data.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp_str)
            .map(|dt| dt.timestamp_millis())
            .ok();

        let parse_entries = |arr: &Value| -> Vec<OrderBookEntry> {
            arr.as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|e| {
                            let price = e
                                .get("px")
                                .and_then(|v| v.as_f64())
                                .and_then(|f| Decimal::try_from(f).ok())?;
                            let amount = e
                                .get("qty")
                                .and_then(|v| v.as_f64())
                                .and_then(|f| Decimal::try_from(f).ok())?;
                            Some(OrderBookEntry { price, amount })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        let bids = parse_entries(data.get("bids").unwrap_or(&Value::Null));
        let asks = parse_entries(data.get("asks").unwrap_or(&Value::Null));

        let is_snapshot = event == "snapshot";

        let orderbook = if is_snapshot {
            // Full snapshot
            OrderBook {
                symbol: symbol.clone(),
                bids,
                asks,
                timestamp,
                datetime: Some(timestamp_str.to_string()),
                nonce: None,
                checksum: None,
            }
        } else {
            // Incremental update - apply to cache
            let mut cache = orderbook_cache.write().await;
            let existing = cache.entry(symbol.clone()).or_insert_with(|| OrderBook {
                symbol: symbol.clone(),
                bids: Vec::new(),
                asks: Vec::new(),
                timestamp: None,
                datetime: None,
                nonce: None,
                checksum: None,
            });

            // Apply bid deltas
            for entry in &bids {
                if entry.amount == Decimal::ZERO {
                    existing.bids.retain(|b| b.price != entry.price);
                } else if let Some(pos) = existing.bids.iter().position(|b| b.price == entry.price)
                {
                    existing.bids[pos].amount = entry.amount;
                } else {
                    existing.bids.push(entry.clone());
                    existing.bids.sort_by(|a, b| b.price.cmp(&a.price));
                }
            }

            // Apply ask deltas
            for entry in &asks {
                if entry.amount == Decimal::ZERO {
                    existing.asks.retain(|a| a.price != entry.price);
                } else if let Some(pos) = existing.asks.iter().position(|a| a.price == entry.price)
                {
                    existing.asks[pos].amount = entry.amount;
                } else {
                    existing.asks.push(entry.clone());
                    existing.asks.sort_by(|a, b| a.price.cmp(&b.price));
                }
            }

            existing.timestamp = timestamp;
            existing.datetime = Some(timestamp_str.to_string());

            existing.clone()
        };

        // Update cache for snapshots
        if is_snapshot {
            let mut cache = orderbook_cache.write().await;
            cache.insert(symbol.clone(), orderbook.clone());
        }

        let subs = subscriptions.read().await;
        if let Some(tx) = subs.get(&message_hash) {
            let event = WsOrderBookEvent {
                symbol: symbol.clone(),
                order_book: orderbook,
                is_snapshot,
            };
            let _ = tx.send(WsMessage::OrderBook(event));
        }
    }

    /// Handle OHLCV messages
    async fn handle_ohlcv(
        data: &Value,
        event: &str,
        subscriptions: &Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
        ohlcv_cache: &Arc<RwLock<HashMap<String, Vec<OHLCV>>>>,
        ohlcv_timeframes: &Arc<RwLock<HashMap<String, Timeframe>>>,
    ) {
        if event != "updated" {
            return;
        }

        let market_id = data.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = market_id.replace("-", "/");
        let message_hash = format!("ohlcv:{symbol}");

        // Parse OHLCV array: [timestamp, high, low, open, close, volume]
        if let Some(price_arr) = data.get("price").and_then(|v| v.as_array()) {
            if price_arr.len() >= 6 {
                let ohlcv = OHLCV {
                    timestamp: price_arr[0].as_i64().unwrap_or(0),
                    open: price_arr[3]
                        .as_f64()
                        .and_then(|f| Decimal::try_from(f).ok())
                        .unwrap_or_default(),
                    high: price_arr[1]
                        .as_f64()
                        .and_then(|f| Decimal::try_from(f).ok())
                        .unwrap_or_default(),
                    low: price_arr[2]
                        .as_f64()
                        .and_then(|f| Decimal::try_from(f).ok())
                        .unwrap_or_default(),
                    close: price_arr[4]
                        .as_f64()
                        .and_then(|f| Decimal::try_from(f).ok())
                        .unwrap_or_default(),
                    volume: price_arr[5]
                        .as_f64()
                        .and_then(|f| Decimal::try_from(f).ok())
                        .unwrap_or_default(),
                };

                // Update cache
                {
                    let mut cache = ohlcv_cache.write().await;
                    let candles = cache.entry(symbol.clone()).or_insert_with(Vec::new);

                    // Replace or append based on timestamp
                    if let Some(pos) = candles.iter().position(|c| c.timestamp == ohlcv.timestamp) {
                        candles[pos] = ohlcv.clone();
                    } else {
                        candles.push(ohlcv.clone());
                        // Keep only last 1000 candles
                        if candles.len() > 1000 {
                            candles.remove(0);
                        }
                    }
                }

                // Get the actual timeframe for this symbol
                let timeframe = {
                    let tf = ohlcv_timeframes.read().await;
                    tf.get(&symbol).copied().unwrap_or(Timeframe::Minute1)
                };

                let subs = subscriptions.read().await;
                if let Some(tx) = subs.get(&message_hash) {
                    let event = WsOhlcvEvent {
                        symbol: symbol.clone(),
                        timeframe,
                        ohlcv,
                    };
                    let _ = tx.send(WsMessage::Ohlcv(event));
                }
            }
        }
    }
}

impl Default for BlockchainComWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BlockchainComWs {
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Build request with Origin header
        let request = WsRequest::builder()
            .uri(WS_URL)
            .header("Origin", ORIGIN_HEADER)
            .header("Host", "ws.blockchain.info")
            .body(())
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("Failed to build request: {e}"),
            })?;

        let (ws, _) = connect_async_with_config(request, None, false)
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_URL.to_string(),
                message: format!("Failed to connect: {e}"),
            })?;

        {
            let mut ws_guard = self.ws_stream.write().await;
            *ws_guard = Some(ws);
        }

        self.start_message_loop();

        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        let ws_guard = self.ws_stream.read().await;
        ws_guard.is_some()
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        self.internal_close().await
    }

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self.format_symbol(symbol);
        let message_hash = format!("ticker:{symbol}");

        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(message_hash, tx);
        }

        self.subscribe("ticker", Some(&market_id), None).await?;

        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self.format_symbol(symbol);
        let message_hash = format!("trades:{symbol}");

        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(message_hash, tx);
        }

        self.subscribe("trades", Some(&market_id), None).await?;

        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self.format_symbol(symbol);
        let message_hash = format!("orderbook:{symbol}");

        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(message_hash, tx);
        }

        // Use l2 channel for order book (l3 is also available for more granular data)
        self.subscribe("l2", Some(&market_id), None).await?;

        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self.format_symbol(symbol);
        let message_hash = format!("ohlcv:{symbol}");

        let granularity =
            self.timeframe_to_granularity(timeframe)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: format!("Unsupported timeframe: {timeframe:?}"),
                })?;

        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(message_hash, tx);
        }

        // Store the timeframe for this symbol
        {
            let mut tf = self.ohlcv_timeframes.write().await;
            tf.insert(symbol.to_string(), timeframe);
        }

        let extra_params = json!({ "granularity": granularity });
        self.subscribe("prices", Some(&market_id), Some(extra_params))
            .await?;

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let ws = BlockchainComWs::new();
        assert_eq!(ws.format_symbol("BTC/USD"), "BTC-USD");
        assert_eq!(ws.format_symbol("ETH/USDT"), "ETH-USDT");
        assert_eq!(ws.parse_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(ws.parse_symbol("ETH-USDT"), "ETH/USDT");
    }

    #[test]
    fn test_timeframe_conversion() {
        let ws = BlockchainComWs::new();
        assert_eq!(ws.timeframe_to_granularity(Timeframe::Minute1), Some(60));
        assert_eq!(ws.timeframe_to_granularity(Timeframe::Minute5), Some(300));
        assert_eq!(ws.timeframe_to_granularity(Timeframe::Minute15), Some(900));
        assert_eq!(ws.timeframe_to_granularity(Timeframe::Hour1), Some(3600));
        assert_eq!(ws.timeframe_to_granularity(Timeframe::Hour6), Some(21600));
        assert_eq!(ws.timeframe_to_granularity(Timeframe::Day1), Some(86400));
    }

    #[test]
    fn test_new_client() {
        let ws = BlockchainComWs::new();
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_with_credentials() {
        let ws = BlockchainComWs::with_credentials("test_secret".to_string());
        assert_eq!(ws.api_secret, Some("test_secret".to_string()));
    }
}
