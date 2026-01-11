//! Toobit WebSocket Implementation
//!
//! Real-time data streaming for Toobit exchange

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, TakerOrMaker,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://stream.toobit.com/quote/ws/v1";

/// Toobit WebSocket client
pub struct ToobitWs {
    config: ExchangeConfig,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: AtomicI64,
    order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
}

/// Toobit ticker data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct ToobitWsTicker {
    t: Option<i64>,       // timestamp
    s: Option<String>,    // symbol
    o: Option<String>,    // open
    h: Option<String>,    // high
    l: Option<String>,    // low
    c: Option<String>,    // close (last)
    v: Option<String>,    // volume
    qv: Option<String>,   // quote volume
    m: Option<String>,    // change
    e: Option<i64>,       // event type (integer, not string)
}

/// Toobit order book data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct ToobitWsOrderBook {
    e: Option<i64>,              // event type
    t: Option<i64>,              // timestamp
    s: Option<String>,           // symbol
    a: Option<Vec<Vec<String>>>, // asks [price, qty]
    b: Option<Vec<Vec<String>>>, // bids [price, qty]
}

/// Toobit trade data from WebSocket
/// Note: symbol is NOT in the trade data item, it's in the outer wrapper
#[derive(Debug, Deserialize, Serialize)]
struct ToobitWsTrade {
    v: Option<String>,    // trade id
    t: Option<i64>,       // timestamp
    p: Option<String>,    // price
    q: Option<String>,    // quantity
    m: Option<bool>,      // is buyer maker (true = sell, false = buy)
}

/// WebSocket message wrapper
#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct ToobitWsMessage {
    symbol: Option<String>,    // symbol from wrapper (e.g., "BTCUSDT")
    topic: Option<String>,
    event: Option<String>,
    params: Option<ToobitWsParams>,
    data: Option<serde_json::Value>,
    f: Option<bool>,      // first update
    sendTime: Option<i64>,
    shared: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct ToobitWsParams {
    binary: Option<String>,
    realtimeInterval: Option<String>,
    klineType: Option<String>,
}

impl ToobitWs {
    /// Create new Toobit WebSocket client
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: AtomicI64::new(1),
            order_books: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        Self::new(config)
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
    }

    /// Get next request ID
    fn next_request_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Convert symbol to Toobit market ID
    fn to_market_id(symbol: &str) -> String {
        // BTC/USDT -> BTCUSDT
        symbol.replace('/', "")
    }

    /// Convert market ID to unified symbol
    fn to_unified_symbol(market_id: &str) -> String {
        // BTCUSDT -> BTC/USDT (simplified - assumes USDT pairs)
        if let Some(base) = market_id.strip_suffix("USDT") {
            format!("{base}/USDT")
        } else if let Some(base) = market_id.strip_suffix("USDC") {
            format!("{base}/USDC")
        } else if let Some(base) = market_id.strip_suffix("BTC") {
            format!("{base}/BTC")
        } else {
            market_id.to_string()
        }
    }

    /// Parse ticker message
    fn parse_ticker(data: &ToobitWsTicker) -> Option<WsTickerEvent> {
        let symbol_raw = data.s.as_ref()?;
        let symbol = Self::to_unified_symbol(symbol_raw);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.l.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.o.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data.c.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.c.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: data.m.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            percentage: None,
            average: None,
            base_volume: data.v.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data.qv.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTickerEvent { symbol, ticker })
    }

    /// Parse order book message
    fn parse_order_book(data: &ToobitWsOrderBook) -> Option<WsOrderBookEvent> {
        let symbol_raw = data.s.as_ref()?;
        let symbol = Self::to_unified_symbol(symbol_raw);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(ref bid_data) = data.b {
            for entry in bid_data {
                if entry.len() >= 2 {
                    if let (Ok(price), Ok(amount)) = (
                        Decimal::from_str(&entry[0]),
                        Decimal::from_str(&entry[1]),
                    ) {
                        bids.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        if let Some(ref ask_data) = data.a {
            for entry in ask_data {
                if entry.len() >= 2 {
                    if let (Ok(price), Ok(amount)) = (
                        Decimal::from_str(&entry[0]),
                        Decimal::from_str(&entry[1]),
                    ) {
                        asks.push(OrderBookEntry { price, amount });
                    }
                }
            }
        }

        let order_book = OrderBook {
            symbol: symbol.clone(),
            bids,
            asks,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
        };

        Some(WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,
        })
    }

    /// Parse trade message
    /// Note: symbol comes from the wrapper message, not from trade data
    fn parse_trade(data: &ToobitWsTrade, symbol_raw: &str) -> Option<WsTradeEvent> {
        let symbol = Self::to_unified_symbol(symbol_raw);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let price = data.p.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();
        let amount = data.q.as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        // m = is buyer maker, so if true, the taker was seller
        let side = if data.m.unwrap_or(false) {
            Some("sell".to_string())
        } else {
            Some("buy".to_string())
        };

        let trade = Trade {
            id: data.v.clone().unwrap_or_else(|| timestamp.to_string()),
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
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTradeEvent {
            symbol,
            trades: vec![trade],
        })
    }

    /// Process WebSocket message
    async fn process_message(
        msg: &str,
        order_books: &RwLock<HashMap<String, OrderBook>>,
    ) -> Option<WsMessage> {
        // Skip ping/pong messages
        if msg == "pong" || msg.contains("\"pong\"") {
            return None;
        }

        // Try to parse as JSON
        let wrapper: ToobitWsMessage = serde_json::from_str(msg).ok()?;

        // Get topic to determine message type
        let topic = wrapper.topic.as_ref()?;

        // Handle different message types based on topic
        if topic.starts_with("realtimes") {
            // Ticker update - data is an array containing ticker objects
            if let Some(ref data_arr) = wrapper.data {
                if let Some(arr) = data_arr.as_array() {
                    for item in arr {
                        if let Ok(ticker_data) = serde_json::from_value::<ToobitWsTicker>(item.clone()) {
                            if let Some(event) = Self::parse_ticker(&ticker_data) {
                                return Some(WsMessage::Ticker(event));
                            }
                        }
                    }
                }
            }
        } else if topic.starts_with("depth") {
            // Order book update - data is an array containing the order book object
            if let Some(ref data_arr) = wrapper.data {
                if let Some(arr) = data_arr.as_array() {
                    for item in arr {
                        if let Ok(book_data) = serde_json::from_value::<ToobitWsOrderBook>(item.clone()) {
                            if let Some(event) = Self::parse_order_book(&book_data) {
                                // Store in cache
                                let symbol = event.symbol.clone();
                                let mut books = order_books.write().await;
                                books.insert(symbol, event.order_book.clone());
                                return Some(WsMessage::OrderBook(event));
                            }
                        }
                    }
                }
            }
        } else if topic.starts_with("trade") {
            // Trade update - symbol is in the wrapper, not in trade data items
            if let Some(ref symbol_raw) = wrapper.symbol {
                if let Some(ref data_arr) = wrapper.data {
                    if let Some(arr) = data_arr.as_array() {
                        for item in arr {
                            if let Ok(trade_data) = serde_json::from_value::<ToobitWsTrade>(item.clone()) {
                                if let Some(event) = Self::parse_trade(&trade_data, symbol_raw) {
                                    return Some(WsMessage::Trade(event));
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Subscribe to stream and return event receiver
    async fn subscribe_stream(
        &mut self,
        topics: Vec<serde_json::Value>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 300, // Long interval to avoid issues
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send subscription messages for each topic
        for topic in &topics {
            ws_client.send(&topic.to_string())?;
        }

        // Event processing task
        // IMPORTANT: Move ws_client INTO the spawned task to keep it alive
        // Otherwise command_tx gets dropped when the function returns,
        // causing command_rx.recv() to return None and trigger reconnection
        let tx = event_tx.clone();
        let order_books = Arc::clone(&self.order_books);
        tokio::spawn(async move {
            // Keep ws_client alive for the duration of this task
            let _ws_client = ws_client;

            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg, &order_books).await {
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

impl Default for ToobitWs {
    fn default() -> Self {
        Self::new(ExchangeConfig::default())
    }
}

#[async_trait]
impl WsExchange for ToobitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);

        let sub_msg = serde_json::json!({
            "topic": "realtimes",
            "event": "sub",
            "symbol": market_id,
            "params": {
                "binary": false
            }
        });

        client.subscribe_stream(vec![sub_msg]).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());

        let topics: Vec<serde_json::Value> = symbols.iter()
            .map(|s| {
                let market_id = Self::to_market_id(s);
                serde_json::json!({
                    "topic": "realtimes",
                    "event": "sub",
                    "symbol": market_id,
                    "params": {
                        "binary": "false"
                    }
                })
            })
            .collect();

        client.subscribe_stream(topics).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);

        let sub_msg = serde_json::json!({
            "topic": "depth",
            "event": "sub",
            "symbol": market_id,
            "params": {
                "binary": false
            }
        });

        client.subscribe_stream(vec![sub_msg]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);

        let sub_msg = serde_json::json!({
            "topic": "trade",
            "event": "sub",
            "symbol": market_id,
            "params": {
                "binary": false
            }
        });

        client.subscribe_stream(vec![sub_msg]).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);

        let interval = match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
            _ => "1h",
        };

        let sub_msg = serde_json::json!({
            "topic": "kline",
            "event": "sub",
            "symbol": market_id,
            "params": {
                "binary": false,
                "klineType": interval
            }
        });

        client.subscribe_stream(vec![sub_msg]).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ref ws) = self.ws_client {
            ws.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_client.is_some()
    }
}
