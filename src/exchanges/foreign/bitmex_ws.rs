//! BitMEX WebSocket Implementation
//!
//! BitMEX WebSocket API for real-time market data and private user data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const WS_BASE_URL: &str = "wss://www.bitmex.com/realtime";
const WS_TESTNET_URL: &str = "wss://testnet.bitmex.com/realtime";

/// BitMEX WebSocket client
pub struct BitmexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
    testnet: bool,
}

impl BitmexWs {
    /// Create new BitMEX WebSocket client
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
            testnet: false,
        }
    }

    /// Create new BitMEX WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
            testnet: false,
        }
    }

    /// Create new BitMEX WebSocket client with testnet flag
    pub fn with_testnet(testnet: bool) -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
            testnet,
        }
    }

    /// Get WebSocket URL based on testnet flag
    fn get_url(&self) -> &'static str {
        if self.testnet {
            WS_TESTNET_URL
        } else {
            WS_BASE_URL
        }
    }

    /// Timeframe to BitMEX tradeBin interval mapping
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Hour1 => "1h",
            Timeframe::Day1 => "1d",
            _ => "1m", // Default to 1 minute
        }
    }

    /// Parse unified symbol from BitMEX symbol
    fn to_unified_symbol(symbol: &str) -> String {
        // BitMEX format: XBTUSD (perpetual), XBTM23 (futures)
        // Convert to unified: BTC/USD:BTC
        if symbol.starts_with("XBT") {
            if symbol == "XBTUSD" || symbol.ends_with("USD") {
                return "BTC/USD:BTC".to_string();
            }
            // Futures contract
            return format!("BTC/USD:BTC:{}", &symbol[3..]);
        }

        if symbol.starts_with("ETH") {
            if symbol == "ETHUSD" {
                return "ETH/USD:ETH".to_string();
            }
            return format!("ETH/USD:ETH:{}", &symbol[3..]);
        }

        // Default: keep as is
        symbol.to_string()
    }

    /// Parse BitMEX symbol to unified symbol
    fn from_unified_symbol(symbol: &str) -> String {
        // Convert unified symbol back to BitMEX format
        // BTC/USD:BTC -> XBTUSD
        // ETH/USD:ETH -> ETHUSD
        if symbol.starts_with("BTC/USD") {
            if symbol == "BTC/USD:BTC" {
                return "XBTUSD".to_string();
            }
            // Extract contract date/type if present
            let parts: Vec<&str> = symbol.split(':').collect();
            if parts.len() > 2 {
                return format!("XBT{}", parts[2]);
            }
            return "XBTUSD".to_string();
        }

        if symbol.starts_with("ETH/USD") {
            if symbol == "ETH/USD:ETH" {
                return "ETHUSD".to_string();
            }
            let parts: Vec<&str> = symbol.split(':').collect();
            if parts.len() > 2 {
                return format!("ETH{}", parts[2]);
            }
            return "ETHUSD".to_string();
        }

        symbol.to_string()
    }

    /// Generate authentication signature
    fn generate_auth_signature(&self, expires: i64) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key and secret required for authentication".into(),
        })?;

        let secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let message = format!("GET/realtime{}", expires);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid secret key: {}", e),
            })?;

        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        Ok(signature)
    }

    /// Authenticate WebSocket connection
    async fn authenticate(&self) -> CcxtResult<()> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required for authentication".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let expires = Utc::now().timestamp() + 60; // 60 seconds from now
        let signature = self.generate_auth_signature(expires)?;

        let auth_msg = serde_json::json!({
            "op": "authKeyExpires",
            "args": [api_key, expires, signature]
        });

        if let Some(client) = &self.ws_client {
            let msg_str = serde_json::to_string(&auth_msg)
                .map_err(|e| CcxtError::ParseError {
                    data_type: "AuthMessage".to_string(),
                    message: e.to_string(),
                })?;
            client.send(&msg_str)?;
        }

        *self.authenticated.write().await = true;
        Ok(())
    }

    /// Parse ticker message
    fn parse_ticker(data: &BitmexInstrumentData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.clone()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price.and_then(|p| Decimal::from_f64_retain(p)),
            low: data.low_price.and_then(|p| Decimal::from_f64_retain(p)),
            bid: data.bid_price.and_then(|p| Decimal::from_f64_retain(p)),
            bid_volume: None,
            ask: data.ask_price.and_then(|p| Decimal::from_f64_retain(p)),
            ask_volume: None,
            vwap: data.vwap.and_then(|p| Decimal::from_f64_retain(p)),
            open: None,
            close: data.last_price.and_then(|p| Decimal::from_f64_retain(p)),
            last: data.last_price.and_then(|p| Decimal::from_f64_retain(p)),
            previous_close: data.prev_close_price.and_then(|p| Decimal::from_f64_retain(p)),
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.map(|v| Decimal::new(v, 0)),
            quote_volume: data.volume_usd.map(|v| Decimal::new(v, 0)),
            index_price: None,
            mark_price: data.mark_price.and_then(|p| Decimal::from_f64_retain(p)),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &BitmexOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter()
            .filter_map(|b| {
                Some(OrderBookEntry {
                    price: Decimal::from_f64_retain(b.price)?,
                    amount: Decimal::new(b.size, 0),
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter()
            .filter_map(|a| {
                Some(OrderBookEntry {
                    price: Decimal::from_f64_retain(a.price)?,
                    amount: Decimal::new(a.size, 0),
                })
            })
            .collect();

        let timestamp = data.timestamp.clone()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// Parse trade message
    fn parse_trades(trades: &[BitmexTradeData]) -> Option<WsTradeEvent> {
        if trades.is_empty() {
            return None;
        }

        let first_trade = &trades[0];
        let symbol = Self::to_unified_symbol(&first_trade.symbol);

        let parsed_trades: Vec<Trade> = trades.iter()
            .map(|t| {
                let timestamp = t.timestamp.clone()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let side = t.side.as_ref().map(|s| s.to_lowercase());
                let price = Decimal::from_f64_retain(t.price).unwrap_or_default();
                let amount = Decimal::new(t.size, 0);

                Trade {
                    id: t.trd_match_id.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side,
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Some(WsTradeEvent {
            symbol,
            trades: parsed_trades,
        })
    }

    /// Parse OHLCV message (tradeBin)
    fn parse_ohlcv(data: &BitmexTradeBinData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);

        let timestamp = data.timestamp.clone()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: Decimal::from_f64_retain(data.open)?,
            high: Decimal::from_f64_retain(data.high)?,
            low: Decimal::from_f64_retain(data.low)?,
            close: Decimal::from_f64_retain(data.close)?,
            volume: Decimal::new(data.volume, 0),
        };

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: BitmexMessage = match serde_json::from_str(msg) {
            Ok(m) => m,
            Err(_) => return None,
        };

        // Handle subscription success
        if json.success == Some(true) {
            if let Some(subscribe) = json.subscribe {
                return Some(WsMessage::Subscribed {
                    channel: subscribe,
                    symbol: None,
                });
            }
        }

        // Handle errors
        if let Some(error) = json.error {
            return Some(WsMessage::Error(error));
        }

        // Handle authentication success
        if json.success == Some(true) && json.request == Some("authKeyExpires".to_string()) {
            return Some(WsMessage::Authenticated);
        }

        // Handle data updates
        if let Some(table) = json.table {
            if let Some(data) = json.data {
                let action = json.action.as_deref();

                match table.as_str() {
                    "instrument" => {
                        if let Ok(instruments) = serde_json::from_value::<Vec<BitmexInstrumentData>>(data) {
                            if let Some(instrument) = instruments.first() {
                                return Some(WsMessage::Ticker(Self::parse_ticker(instrument)));
                            }
                        }
                    }
                    "orderBook10" | "orderBookL2" | "orderBookL2_25" => {
                        // For orderBook updates, we need to handle partial, insert, update, delete actions
                        let is_snapshot = action == Some("partial");

                        if let Ok(book_data) = serde_json::from_value::<BitmexOrderBookData>(data.clone()) {
                            if let Some(symbol) = book_data.bids.first().or(book_data.asks.first()).map(|e| &e.symbol) {
                                let unified_symbol = Self::to_unified_symbol(symbol);
                                return Some(WsMessage::OrderBook(
                                    Self::parse_order_book(&book_data, &unified_symbol, is_snapshot)
                                ));
                            }
                        }
                    }
                    "trade" => {
                        if let Ok(trades) = serde_json::from_value::<Vec<BitmexTradeData>>(data) {
                            if let Some(event) = Self::parse_trades(&trades) {
                                return Some(WsMessage::Trade(event));
                            }
                        }
                    }
                    "tradeBin1m" | "tradeBin5m" | "tradeBin1h" | "tradeBin1d" => {
                        let timeframe = match table.as_str() {
                            "tradeBin1m" => Timeframe::Minute1,
                            "tradeBin5m" => Timeframe::Minute5,
                            "tradeBin1h" => Timeframe::Hour1,
                            "tradeBin1d" => Timeframe::Day1,
                            _ => Timeframe::Minute1,
                        };

                        if let Ok(bins) = serde_json::from_value::<Vec<BitmexTradeBinData>>(data) {
                            if let Some(bin) = bins.first() {
                                if let Some(event) = Self::parse_ohlcv(bin, timeframe) {
                                    return Some(WsMessage::Ohlcv(event));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Subscribe to channel and return event receiver
    async fn subscribe_channel(&mut self, topics: Vec<String>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = self.get_url();

        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Authenticate if config is available
        if self.config.is_some() {
            self.authenticate().await?;
        }

        // Send subscription request
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": topics
        });

        if let Some(client) = &self.ws_client {
            let msg_str = serde_json::to_string(&subscribe_msg)
                .map_err(|e| CcxtError::ParseError {
                    data_type: "SubscribeMessage".to_string(),
                    message: e.to_string(),
                })?;
            client.send(&msg_str)?;
        }

        // Store subscriptions
        for topic in topics {
            self.subscriptions.write().await.insert(topic.clone(), topic);
        }

        // Event processing task
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    }
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for BitmexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitmexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);
        let topic = format!("instrument:{}", bitmex_symbol);
        client.subscribe_channel(vec![topic]).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topics: Vec<String> = symbols
            .iter()
            .map(|s| {
                let bitmex_symbol = Self::from_unified_symbol(s);
                format!("instrument:{}", bitmex_symbol)
            })
            .collect();
        client.subscribe_channel(topics).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);

        // Choose orderBook channel based on limit
        let channel = match limit {
            Some(10) | None => "orderBook10", // Top 10 levels (default)
            Some(25) => "orderBookL2_25",     // Top 25 levels
            _ => "orderBookL2",                // Full order book
        };

        let topic = format!("{}:{}", channel, bitmex_symbol);
        client.subscribe_channel(vec![topic]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);
        let topic = format!("trade:{}", bitmex_symbol);
        client.subscribe_channel(vec![topic]).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("tradeBin{}:{}", interval, bitmex_symbol);
        client.subscribe_channel(vec![topic]).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // BitMEX connects on subscription
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        *self.authenticated.write().await = false;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            client.is_connected().await
        } else {
            false
        }
    }
}

// === BitMEX WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct BitmexMessage {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    subscribe: Option<String>,
    #[serde(default)]
    request: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexInstrumentData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    last_price: Option<f64>,
    #[serde(default)]
    bid_price: Option<f64>,
    #[serde(default)]
    ask_price: Option<f64>,
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    high_price: Option<f64>,
    #[serde(default)]
    low_price: Option<f64>,
    #[serde(default)]
    prev_close_price: Option<f64>,
    #[serde(default)]
    volume: Option<i64>,
    #[serde(default)]
    volume_usd: Option<i64>,
    #[serde(default)]
    vwap: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BitmexOrderBookData {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    bids: Vec<BitmexOrderBookLevel>,
    #[serde(default)]
    asks: Vec<BitmexOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct BitmexOrderBookLevel {
    symbol: String,
    price: f64,
    size: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexTradeData {
    #[serde(default)]
    timestamp: Option<String>,
    symbol: String,
    #[serde(default)]
    side: Option<String>,
    size: i64,
    price: f64,
    #[serde(default)]
    trd_match_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitmexTradeBinData {
    timestamp: Option<String>,
    symbol: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: i64,
}
