//! Deribit WebSocket Implementation
//!
//! Deribit WebSocket API for real-time market data and private user data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_BASE_URL: &str = "wss://www.deribit.com/ws/api/v2";
const WS_TESTNET_URL: &str = "wss://test.deribit.com/ws/api/v2";

/// Deribit WebSocket client
pub struct DeribitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: Arc<RwLock<i64>>,
    testnet: bool,
}

impl DeribitWs {
    /// Create new Deribit WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            testnet: false,
        }
    }

    /// Create new Deribit WebSocket client with testnet flag
    pub fn with_testnet(testnet: bool) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            testnet,
        }
    }

    /// Get next request ID
    async fn next_request_id(&self) -> i64 {
        let mut id = self.request_id.write().await;
        *id += 1;
        *id
    }

    /// Get WebSocket URL based on testnet flag
    fn get_url(&self) -> &'static str {
        if self.testnet {
            WS_TESTNET_URL
        } else {
            WS_BASE_URL
        }
    }

    /// Timeframe to Deribit interval mapping
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute3 => "3",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour2 => "120",
            Timeframe::Hour4 => "240",
            Timeframe::Hour6 => "360",
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "1D",
            _ => "60", // Default to 1 hour
        }
    }

    /// Parse unified symbol from Deribit instrument name
    fn to_unified_symbol(instrument_name: &str) -> String {
        // Deribit format: BTC-PERPETUAL, BTC-25DEC20, BTC-25DEC20-36000-C
        let parts: Vec<&str> = instrument_name.split('-').collect();
        if parts.is_empty() {
            return instrument_name.to_string();
        }

        let base = parts[0];

        if parts.len() == 2 && parts[1] == "PERPETUAL" {
            return format!("{}/USD:{}", base, base);
        }

        if parts.len() == 2 {
            // Future: BTC-25DEC20
            return format!("{}/USD:{}", base, base);
        }

        if parts.len() == 4 {
            // Option: BTC-25DEC20-36000-C
            let strike = parts[2];
            let option_type = parts[3];
            return format!("{}/USD:{}:{}:{}", base, base, strike, option_type);
        }

        instrument_name.to_string()
    }

    /// Parse ticker message
    fn parse_ticker(data: &DeribitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.instrument_name);
        let timestamp = data.timestamp;

        let (high, low, volume, percentage) = if let Some(ref stats) = data.stats {
            (
                stats.high.map(Decimal::from_f64_retain).flatten(),
                stats.low.map(Decimal::from_f64_retain).flatten(),
                stats.volume.map(Decimal::from_f64_retain).flatten(),
                stats.price_change.map(Decimal::from_f64_retain).flatten(),
            )
        } else {
            (None, None, None, None)
        };

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high,
            low,
            bid: data.best_bid_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid_volume: data.best_bid_amount.map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
            ask: data.best_ask_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            ask_volume: data.best_ask_amount.map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
            vwap: None,
            open: None,
            close: data.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            last: data.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            previous_close: None,
            change: None,
            percentage,
            average: None,
            base_volume: volume,
            quote_volume: None,
            index_price: data.index_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            mark_price: data.mark_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &DeribitOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let is_snapshot = data.r#type.as_deref() == Some("snapshot");

        let bids: Vec<OrderBookEntry> = data.bids.iter()
            .filter_map(|b| {
                if b.len() >= 3 {
                    // Format: ["new"|"change"|"delete", price, amount]
                    Some(OrderBookEntry {
                        price: b[1].as_f64().and_then(Decimal::from_f64_retain)?,
                        amount: b[2].as_f64().and_then(Decimal::from_f64_retain)?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter()
            .filter_map(|a| {
                if a.len() >= 3 {
                    Some(OrderBookEntry {
                        price: a[1].as_f64().and_then(Decimal::from_f64_retain)?,
                        amount: a[2].as_f64().and_then(Decimal::from_f64_retain)?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.change_id,
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
    fn parse_trade(trade_data: &DeribitTradeData, symbol: &str) -> Trade {
        let timestamp = trade_data.timestamp;

        let side = match trade_data.direction.as_str() {
            "buy" => Some("buy".to_string()),
            "sell" => Some("sell".to_string()),
            _ => None,
        };

        let price = Decimal::from_f64_retain(trade_data.price).unwrap_or_default();
        let amount = Decimal::from_f64_retain(trade_data.amount).unwrap_or_default();

        Trade {
            id: trade_data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(trade_data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV message
    fn parse_ohlcv(data: &DeribitOhlcvData, _timeframe: Timeframe) -> OHLCV {
        OHLCV {
            timestamp: data.tick,
            open: Decimal::from_f64_retain(data.open).unwrap_or_default(),
            high: Decimal::from_f64_retain(data.high).unwrap_or_default(),
            low: Decimal::from_f64_retain(data.low).unwrap_or_default(),
            close: Decimal::from_f64_retain(data.close).unwrap_or_default(),
            volume: Decimal::from_f64_retain(data.volume).unwrap_or_default(),
        }
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: DeribitMessage = match serde_json::from_str(msg) {
            Ok(m) => m,
            Err(_) => return None,
        };

        // Check for errors
        if let Some(error) = json.error {
            return Some(WsMessage::Error(format!("{}: {}", error.code, error.message)));
        }

        // Handle subscription notifications
        if json.method.as_deref() == Some("subscription") {
            if let Some(params) = json.params {
                if let Some(channel) = params.channel {
                    let parts: Vec<&str> = channel.split('.').collect();
                    let channel_type = parts.first()?;

                    match *channel_type {
                        "ticker" => {
                            if let Ok(data) = serde_json::from_value::<DeribitTickerData>(params.data) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                            }
                        }
                        "book" => {
                            if let Ok(data) = serde_json::from_value::<DeribitOrderBookData>(params.data) {
                                let symbol = Self::to_unified_symbol(&data.instrument_name);
                                return Some(WsMessage::OrderBook(Self::parse_order_book(&data, &symbol)));
                            }
                        }
                        "trades" => {
                            if let Ok(trades) = serde_json::from_value::<Vec<DeribitTradeData>>(params.data) {
                                if let Some(first_trade) = trades.first() {
                                    let symbol = Self::to_unified_symbol(&first_trade.instrument_name);
                                    let parsed_trades: Vec<Trade> = trades.iter()
                                        .map(|t| Self::parse_trade(t, &symbol))
                                        .collect();
                                    return Some(WsMessage::Trade(WsTradeEvent {
                                        symbol,
                                        trades: parsed_trades,
                                    }));
                                }
                            }
                        }
                        "chart" => {
                            if let Ok(data) = serde_json::from_value::<DeribitOhlcvData>(params.data) {
                                // Extract timeframe from channel (e.g., "chart.trades.BTC-PERPETUAL.1")
                                let timeframe = if parts.len() >= 4 {
                                    match parts[3] {
                                        "1" => Timeframe::Minute1,
                                        "3" => Timeframe::Minute3,
                                        "5" => Timeframe::Minute5,
                                        "15" => Timeframe::Minute15,
                                        "30" => Timeframe::Minute30,
                                        "60" => Timeframe::Hour1,
                                        "120" => Timeframe::Hour2,
                                        "240" => Timeframe::Hour4,
                                        "360" => Timeframe::Hour6,
                                        "720" => Timeframe::Hour12,
                                        "1D" => Timeframe::Day1,
                                        _ => Timeframe::Minute1,
                                    }
                                } else {
                                    Timeframe::Minute1
                                };

                                if parts.len() >= 3 {
                                    let symbol = Self::to_unified_symbol(parts[2]);
                                    return Some(WsMessage::Ohlcv(WsOhlcvEvent {
                                        symbol,
                                        timeframe,
                                        ohlcv: Self::parse_ohlcv(&data, timeframe),
                                    }));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        None
    }

    /// Subscribe to channel and return event receiver
    async fn subscribe_channel(&mut self, channels: Vec<String>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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

        // Send subscription request
        let request_id = self.next_request_id().await;
        let subscribe_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "public/subscribe",
            "params": {
                "channels": channels
            }
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
        for channel in channels {
            self.subscriptions.write().await.insert(channel.clone(), channel);
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

impl Default for DeribitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for DeribitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        // Convert symbol to Deribit instrument name (e.g., BTC/USD:BTC -> BTC-PERPETUAL)
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let channel = format!("ticker.{}.100ms", instrument);
        client.subscribe_channel(vec![channel]).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channels: Vec<String> = symbols
            .iter()
            .map(|s| {
                let instrument = s.replace("/USD:", "-").replace("/", "-");
                format!("ticker.{}.100ms", instrument)
            })
            .collect();
        client.subscribe_channel(channels).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let depth = limit.unwrap_or(20);
        // Format: book.{instrument}.{group}.{depth}.{interval}
        // Using "none" for group, depth for limit, "100ms" for interval
        let channel = format!("book.{}.none.{}.100ms", instrument, depth);
        client.subscribe_channel(vec![channel]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let channel = format!("trades.{}.100ms", instrument);
        client.subscribe_channel(vec![channel]).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let interval = Self::format_interval(timeframe);
        let channel = format!("chart.trades.{}.{}", instrument, interval);
        client.subscribe_channel(vec![channel]).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Deribit connects on subscription
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
}

// === Deribit WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct DeribitMessage {
    jsonrpc: String,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<DeribitParams>,
    #[serde(default)]
    error: Option<DeribitError>,
}

#[derive(Debug, Deserialize)]
struct DeribitError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct DeribitParams {
    #[serde(default)]
    channel: Option<String>,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitTickerData {
    instrument_name: String,
    timestamp: i64,
    #[serde(default)]
    best_bid_price: Option<f64>,
    #[serde(default)]
    best_bid_amount: Option<f64>,
    #[serde(default)]
    best_ask_price: Option<f64>,
    #[serde(default)]
    best_ask_amount: Option<f64>,
    #[serde(default)]
    last_price: Option<f64>,
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    index_price: Option<f64>,
    #[serde(default)]
    stats: Option<DeribitStats>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitStats {
    #[serde(default)]
    high: Option<f64>,
    #[serde(default)]
    low: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    price_change: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOrderBookData {
    r#type: Option<String>, // "snapshot" or "change"
    instrument_name: String,
    timestamp: i64,
    #[serde(default)]
    change_id: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<serde_json::Value>>, // [["new"|"change"|"delete", price, amount]]
    #[serde(default)]
    asks: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitTradeData {
    trade_id: String,
    instrument_name: String,
    timestamp: i64,
    direction: String, // "buy" or "sell"
    price: f64,
    amount: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOhlcvData {
    tick: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DeribitWs::to_unified_symbol("BTC-PERPETUAL"), "BTC/USD:BTC");
        assert_eq!(DeribitWs::to_unified_symbol("ETH-PERPETUAL"), "ETH/USD:ETH");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(DeribitWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(DeribitWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(DeribitWs::format_interval(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_deribit_ws_creation() {
        let ws = DeribitWs::new();
        assert_eq!(ws.testnet, false);

        let ws_testnet = DeribitWs::with_testnet(true);
        assert_eq!(ws_testnet.testnet, true);
    }
}
