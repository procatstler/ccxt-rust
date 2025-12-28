//! Bitstamp WebSocket Implementation
//!
//! Bitstamp real-time data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws.bitstamp.net";

/// Bitstamp WebSocket client
pub struct BitstampWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BitstampWs {
    /// Create new Bitstamp WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Convert symbol to Bitstamp WebSocket format (BTC/USD -> btcusd)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Convert Bitstamp symbol to unified format (btcusd -> BTC/USD)
    fn to_unified_symbol(bitstamp_symbol: &str) -> String {
        // Simple heuristic: split into base/quote based on common patterns
        let upper = bitstamp_symbol.to_uppercase();

        // Common quote currencies
        if upper.ends_with("USD") {
            let base = &upper[..upper.len() - 3];
            return format!("{}/USD", base);
        } else if upper.ends_with("USDT") {
            let base = &upper[..upper.len() - 4];
            return format!("{}/USDT", base);
        } else if upper.ends_with("EUR") {
            let base = &upper[..upper.len() - 3];
            return format!("{}/EUR", base);
        } else if upper.ends_with("BTC") {
            let base = &upper[..upper.len() - 3];
            return format!("{}/BTC", base);
        } else if upper.ends_with("ETH") {
            let base = &upper[..upper.len() - 3];
            return format!("{}/ETH", base);
        }

        bitstamp_symbol.to_uppercase()
    }

    /// Parse ticker message
    fn parse_ticker(data: &BitstampTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: None,
            vwap: data.vwap.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: None,
            last: data.last.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &BitstampOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.microtimestamp
            .map(|mt| mt / 1000)
            .or_else(|| Some(Utc::now().timestamp_millis()))
            .unwrap();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&e[0]).ok()?,
                        amount: Decimal::from_str(&e[1]).ok()?,
                    })
                } else {
                    None
                }
            }).collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// Parse diff order book message
    fn parse_order_book_diff(data: &BitstampOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let mut event = Self::parse_order_book(data, symbol);
        event.is_snapshot = false;
        event
    }

    /// Parse trade message
    fn parse_trade(data: &BitstampTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.microtimestamp
            .map(|mt| mt / 1000)
            .or_else(|| Some(Utc::now().timestamp_millis()))
            .unwrap();

        let price = Decimal::from_str(&data.price.to_string()).unwrap_or_default();
        let amount = Decimal::from_str(&data.amount.to_string()).unwrap_or_default();

        let trade = Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: Some(match data.trade_type {
                0 => "buy".to_string(),
                1 => "sell".to_string(),
                _ => "unknown".to_string(),
            }),
            side: Some(match data.trade_type {
                0 => "buy".to_string(),
                1 => "sell".to_string(),
                _ => "unknown".to_string(),
            }),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse the JSON message
        let json: BitstampWsMessage = serde_json::from_str(msg).ok()?;

        match json.event.as_str() {
            "bts:subscription_succeeded" => {
                Some(WsMessage::Subscribed {
                    channel: json.channel.clone(),
                    symbol: None,
                })
            }
            "data" => {
                let channel = json.channel.as_str();

                // Extract symbol from channel name
                // Format: "live_ticker_btcusd", "order_book_btcusd", "live_trades_btcusd"
                let symbol = if let Some(idx) = channel.rfind('_') {
                    &channel[idx + 1..]
                } else {
                    ""
                };

                if channel.starts_with("live_ticker") {
                    if let Some(data) = &json.data {
                        if let Ok(ticker_data) = serde_json::from_value::<BitstampTickerData>(data.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    }
                } else if channel.starts_with("order_book") {
                    if let Some(data) = &json.data {
                        if let Ok(ob_data) = serde_json::from_value::<BitstampOrderBookData>(data.clone()) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, symbol)));
                        }
                    }
                } else if channel.starts_with("diff_order_book") {
                    if let Some(data) = &json.data {
                        if let Ok(ob_data) = serde_json::from_value::<BitstampOrderBookData>(data.clone()) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book_diff(&ob_data, symbol)));
                        }
                    }
                } else if channel.starts_with("live_trades") {
                    if let Some(data) = &json.data {
                        if let Ok(trade_data) = serde_json::from_value::<BitstampTradeData>(data.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                        }
                    }
                }

                None
            }
            _ => None,
        }
    }

    /// Subscribe to a channel and return event stream
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbol: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let formatted_symbol = Self::format_symbol(symbol);
        let channel_name = format!("{}_{}", channel, formatted_symbol);

        let subscribe_msg = BitstampSubscribeMessage {
            event: "bts:subscribe".to_string(),
            data: BitstampSubscribeData {
                channel: channel_name.clone(),
            },
        };

        ws_client.send(&serde_json::to_string(&subscribe_msg).unwrap())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{}:{}", channel, formatted_symbol);
            self.subscriptions.write().await.insert(key, channel_name);
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

#[async_trait]
impl WsExchange for BitstampWs {
    /// Subscribe to ticker updates
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = BitstampWs::new();
        ws.subscribe_stream("live_ticker", symbol).await
    }

    /// Subscribe to order book updates
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = BitstampWs::new();
        ws.subscribe_stream("order_book", symbol).await
    }

    /// Subscribe to trade updates
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = BitstampWs::new();
        ws.subscribe_stream("live_trades", symbol).await
    }

    /// Subscribe to OHLCV updates (not supported by Bitstamp WebSocket API)
    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOhlcv".into(),
        })
    }

    /// Connect to WebSocket
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });

            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    /// Close WebSocket connection
    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }

    /// Check if WebSocket is connected
    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

impl Default for BitstampWs {
    fn default() -> Self {
        Self::new()
    }
}

// ===== Bitstamp WebSocket Message Structures =====

#[derive(Debug, Deserialize)]
struct BitstampWsMessage {
    event: String,
    channel: String,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct BitstampSubscribeMessage {
    event: String,
    data: BitstampSubscribeData,
}

#[derive(Debug, Serialize)]
struct BitstampSubscribeData {
    channel: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampTickerData {
    timestamp: Option<i64>,
    last: Option<String>,
    high: Option<String>,
    low: Option<String>,
    vwap: Option<String>,
    volume: Option<String>,
    bid: Option<String>,
    ask: Option<String>,
    open: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampOrderBookData {
    microtimestamp: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampTradeData {
    id: u64,
    microtimestamp: Option<i64>,
    amount: f64,
    price: f64,
    #[serde(rename = "type")]
    trade_type: i32, // 0 = buy, 1 = sell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitstampWs::format_symbol("BTC/USD"), "btcusd");
        assert_eq!(BitstampWs::format_symbol("ETH/USDT"), "ethusdt");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitstampWs::to_unified_symbol("btcusd"), "BTC/USD");
        assert_eq!(BitstampWs::to_unified_symbol("ethusdt"), "ETH/USDT");
        assert_eq!(BitstampWs::to_unified_symbol("ethbtc"), "ETH/BTC");
    }

    #[test]
    fn test_ws_client_creation() {
        let ws = BitstampWs::new();
        assert!(ws.ws_client.is_none());
    }
}
