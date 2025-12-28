//! Alpaca WebSocket Implementation
//!
//! Alpaca WebSocket API for real-time crypto market data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_CRYPTO_URL: &str = "wss://stream.data.alpaca.markets/v1beta3/crypto";
const WS_STOCKS_URL: &str = "wss://stream.data.alpaca.markets/v2";

/// Alpaca WebSocket client
pub struct AlpacaWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: Arc<RwLock<i64>>,
    default_loc: String,
    default_source: String,
    is_crypto: bool,
}

impl AlpacaWs {
    /// Create new Alpaca WebSocket client for crypto
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            default_loc: "us".to_string(),
            default_source: "iex".to_string(),
            is_crypto: true,
        }
    }

    /// Create new Alpaca WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            default_loc: "us".to_string(),
            default_source: "iex".to_string(),
            is_crypto: true,
        }
    }

    /// Create new Alpaca WebSocket client for stocks
    pub fn with_stocks(config: ExchangeConfig, source: &str) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            default_loc: "us".to_string(),
            default_source: source.to_string(),
            is_crypto: false,
        }
    }

    /// Get next request ID
    async fn next_request_id(&self) -> i64 {
        let mut id = self.request_id.write().await;
        *id += 1;
        *id
    }

    /// Get WebSocket URL based on market type
    fn get_url(&self) -> String {
        if self.is_crypto {
            format!("{}/{}", WS_CRYPTO_URL, self.default_loc)
        } else {
            format!("{}/{}", WS_STOCKS_URL, self.default_source)
        }
    }

    /// Convert unified symbol to Alpaca market ID (BTC/USD -> BTCUSD)
    fn symbol_to_market_id(symbol: &str) -> String {
        symbol.replace('/', "")
    }

    /// Convert Alpaca market ID to unified symbol (BTCUSD -> BTC/USD)
    fn market_id_to_symbol(market_id: &str) -> String {
        if market_id.contains('/') {
            market_id.to_string()
        } else if market_id.ends_with("USD") && market_id != "USD" {
            let base = &market_id[..market_id.len() - 3];
            format!("{}/USD", base)
        } else if market_id.ends_with("USDT") {
            let base = &market_id[..market_id.len() - 4];
            format!("{}/USDT", base)
        } else {
            market_id.to_string()
        }
    }

    /// Parse Alpaca ticker (quote) message
    fn parse_ticker(data: &AlpacaQuoteMsg) -> WsTickerEvent {
        let symbol = Self::market_id_to_symbol(&data.symbol);
        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.t)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(data.t.clone()),
            high: None,
            low: None,
            bid: data.bp.and_then(Decimal::from_f64_retain),
            bid_volume: data.bs.and_then(Decimal::from_f64_retain),
            ask: data.ap.and_then(Decimal::from_f64_retain),
            ask_volume: data.as_.and_then(Decimal::from_f64_retain),
            vwap: None,
            open: None,
            close: None,
            last: None,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse Alpaca order book message
    fn parse_order_book(data: &AlpacaOrderBookMsg, symbol: &str) -> WsOrderBookEvent {
        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.t)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.b.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: entry[0].as_f64().and_then(Decimal::from_f64_retain)?,
                        amount: entry[1].as_f64().and_then(Decimal::from_f64_retain)?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.a.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: entry[0].as_f64().and_then(Decimal::from_f64_retain)?,
                        amount: entry[1].as_f64().and_then(Decimal::from_f64_retain)?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(data.t.clone()),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: data.r,
        }
    }

    /// Parse Alpaca trade message
    fn parse_trade(data: &AlpacaTradeMsg) -> WsTradeEvent {
        let symbol = Self::market_id_to_symbol(&data.symbol);
        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.t)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

        let side = data.tks.as_ref().and_then(|tks| {
            match tks.as_str() {
                "B" => Some("buy".to_string()),
                "S" => Some("sell".to_string()),
                _ => None,
            }
        });

        let price = Decimal::from_f64_retain(data.p).unwrap_or_default();
        let amount = Decimal::from_f64_retain(data.s).unwrap_or_default();

        let trade = Trade {
            id: data.i.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(data.t.clone()),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// Parse Alpaca bar (OHLCV) message
    fn parse_ohlcv(data: &AlpacaBarMsg, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::market_id_to_symbol(&data.symbol);
        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.t)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: Decimal::from_f64_retain(data.o).unwrap_or_default(),
            high: Decimal::from_f64_retain(data.h).unwrap_or_default(),
            low: Decimal::from_f64_retain(data.l).unwrap_or_default(),
            close: Decimal::from_f64_retain(data.c).unwrap_or_default(),
            volume: Decimal::from_f64_retain(data.v).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Parse as array of messages
        let messages: Vec<AlpacaMessage> = match serde_json::from_str(msg) {
            Ok(m) => m,
            Err(_) => {
                // Try parsing as single message
                if let Ok(single) = serde_json::from_str::<AlpacaMessage>(msg) {
                    vec![single]
                } else {
                    return None;
                }
            }
        };

        for message in messages {
            match message.msg_type.as_str() {
                // Connection success
                "success" => {
                    if let Some(msg_value) = message.msg {
                        if msg_value == "authenticated" {
                            return Some(WsMessage::Authenticated);
                        } else if msg_value == "connected" {
                            return Some(WsMessage::Connected);
                        }
                    }
                }
                // Error
                "error" => {
                    let error_msg = message.msg.unwrap_or_else(|| "Unknown error".to_string());
                    return Some(WsMessage::Error(error_msg));
                }
                // Quote (ticker)
                "q" => {
                    if let Ok(data) = serde_json::from_value::<AlpacaQuoteMsg>(serde_json::to_value(&message).unwrap_or_default()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                    }
                }
                // Trade
                "t" => {
                    if let Ok(data) = serde_json::from_value::<AlpacaTradeMsg>(serde_json::to_value(&message).unwrap_or_default()) {
                        return Some(WsMessage::Trade(Self::parse_trade(&data)));
                    }
                }
                // Order book
                "o" => {
                    if let Ok(data) = serde_json::from_value::<AlpacaOrderBookMsg>(serde_json::to_value(&message).unwrap_or_default()) {
                        let symbol = Self::market_id_to_symbol(&data.symbol);
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&data, &symbol)));
                    }
                }
                // Bar (OHLCV)
                "b" => {
                    if let Ok(data) = serde_json::from_value::<AlpacaBarMsg>(serde_json::to_value(&message).unwrap_or_default()) {
                        let tf = timeframe.unwrap_or(Timeframe::Minute1);
                        return Some(WsMessage::Ohlcv(Self::parse_ohlcv(&data, tf)));
                    }
                }
                // Subscription confirmation
                "subscription" => {
                    // Subscription acknowledged, continue
                }
                _ => {}
            }
        }

        None
    }

    /// Subscribe to channels and return event receiver
    async fn subscribe_channels(
        &mut self,
        channels: HashMap<&str, Vec<String>>,
        timeframe: Option<Timeframe>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = self.get_url();

        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Send authentication message
        if let Some(ref config) = self.config {
            let api_key = config.api_key().unwrap_or_default();
            let api_secret = config.api_secret().unwrap_or_default();

            let auth_msg = serde_json::json!({
                "action": "auth",
                "key": api_key,
                "secret": api_secret
            });

            if let Some(client) = &self.ws_client {
                let msg_str = serde_json::to_string(&auth_msg)
                    .map_err(|e| CcxtError::ParseError {
                        data_type: "AuthMessage".to_string(),
                        message: e.to_string(),
                    })?;
                client.send(&msg_str)?;
            }

            // Wait for authentication
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        // Send subscription message
        let mut subscribe_msg = serde_json::json!({
            "action": "subscribe"
        });

        for (channel, symbols) in &channels {
            subscribe_msg[channel] = serde_json::json!(symbols);
        }

        if let Some(client) = &self.ws_client {
            let msg_str = serde_json::to_string(&subscribe_msg)
                .map_err(|e| CcxtError::ParseError {
                    data_type: "SubscribeMessage".to_string(),
                    message: e.to_string(),
                })?;
            client.send(&msg_str)?;
        }

        // Store subscriptions
        for (channel, symbols) in channels {
            for symbol in symbols {
                let key = format!("{}:{}", channel, symbol);
                self.subscriptions.write().await.insert(key.clone(), symbol.clone());
            }
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
                        if let Some(ws_msg) = Self::process_message(&msg, timeframe) {
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

impl Default for AlpacaWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for AlpacaWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let market_id = Self::symbol_to_market_id(symbol);

        let mut channels = HashMap::new();
        channels.insert("quotes", vec![market_id]);

        client.subscribe_channels(channels, None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let market_ids: Vec<String> = symbols
            .iter()
            .map(|s| Self::symbol_to_market_id(s))
            .collect();

        let mut channels = HashMap::new();
        channels.insert("quotes", market_ids);

        client.subscribe_channels(channels, None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let market_id = Self::symbol_to_market_id(symbol);

        let mut channels = HashMap::new();
        channels.insert("orderbooks", vec![market_id]);

        client.subscribe_channels(channels, None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let market_id = Self::symbol_to_market_id(symbol);

        let mut channels = HashMap::new();
        channels.insert("trades", vec![market_id]);

        client.subscribe_channels(channels, None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let market_id = Self::symbol_to_market_id(symbol);

        let mut channels = HashMap::new();
        channels.insert("bars", vec![market_id]);

        client.subscribe_channels(channels, Some(timeframe)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Alpaca connects on subscription
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            client.is_connected().await
        } else {
            false
        }
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // Authentication happens in subscribe_channels
        Ok(())
    }
}

impl Clone for AlpacaWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::clone(&self.request_id),
            default_loc: self.default_loc.clone(),
            default_source: self.default_source.clone(),
            is_crypto: self.is_crypto,
        }
    }
}

// === Alpaca WebSocket Message Types ===

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaMessage {
    #[serde(rename = "T")]
    msg_type: String, // Message type
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    code: Option<i32>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaQuoteMsg {
    #[serde(rename = "T")]
    msg_type: String, // "q"
    #[serde(rename = "S")]
    symbol: String, // Symbol
    t: String, // Timestamp
    #[serde(default)]
    bp: Option<f64>, // Bid price
    #[serde(default)]
    bs: Option<f64>, // Bid size
    #[serde(default)]
    ap: Option<f64>, // Ask price
    #[serde(default, rename = "as")]
    as_: Option<f64>, // Ask size
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaTradeMsg {
    #[serde(rename = "T")]
    msg_type: String, // "t"
    #[serde(rename = "S")]
    symbol: String, // Symbol
    t: String, // Timestamp
    i: i64,    // Trade ID
    p: f64,    // Price
    s: f64,    // Size
    #[serde(default)]
    tks: Option<String>, // Taker side (B/S)
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaOrderBookMsg {
    #[serde(rename = "T")]
    msg_type: String, // "o"
    #[serde(rename = "S")]
    symbol: String, // Symbol
    t: String, // Timestamp
    r: bool,   // Reset (snapshot)
    #[serde(default)]
    b: Vec<Vec<serde_json::Value>>, // Bids [[price, size], ...]
    #[serde(default)]
    a: Vec<Vec<serde_json::Value>>, // Asks [[price, size], ...]
}

#[derive(Debug, Deserialize, Serialize)]
struct AlpacaBarMsg {
    #[serde(rename = "T")]
    msg_type: String, // "b"
    #[serde(rename = "S")]
    symbol: String, // Symbol
    t: String, // Timestamp
    o: f64,    // Open
    h: f64,    // High
    l: f64,    // Low
    c: f64,    // Close
    v: f64,    // Volume
    #[serde(default)]
    n: Option<i64>, // Trade count
    #[serde(default)]
    vw: Option<f64>, // VWAP
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        assert_eq!(AlpacaWs::symbol_to_market_id("BTC/USD"), "BTCUSD");
        assert_eq!(AlpacaWs::symbol_to_market_id("ETH/USDT"), "ETHUSDT");

        assert_eq!(AlpacaWs::market_id_to_symbol("BTCUSD"), "BTC/USD");
        assert_eq!(AlpacaWs::market_id_to_symbol("ETHUSDT"), "ETH/USDT");
    }

    #[test]
    fn test_alpaca_ws_creation() {
        let ws = AlpacaWs::new();
        assert!(ws.is_crypto);
        assert_eq!(ws.default_loc, "us");
    }

    #[test]
    fn test_alpaca_ws_with_config() {
        let config = ExchangeConfig::default();
        let ws = AlpacaWs::with_config(config);
        assert!(ws.is_crypto);
        assert!(ws.config.is_some());
    }

    #[test]
    fn test_get_url_crypto() {
        let ws = AlpacaWs::new();
        let url = ws.get_url();
        assert_eq!(url, "wss://stream.data.alpaca.markets/v1beta3/crypto/us");
    }

    #[test]
    fn test_get_url_stocks() {
        let config = ExchangeConfig::default();
        let ws = AlpacaWs::with_stocks(config, "sip");
        let url = ws.get_url();
        assert_eq!(url, "wss://stream.data.alpaca.markets/v2/sip");
    }
}
