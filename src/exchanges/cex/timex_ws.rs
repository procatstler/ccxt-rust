//! Timex WebSocket Implementation
//!
//! Timex cryptocurrency exchange WebSocket streams

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://plasma-relay-backend.timex.io/socket/websocket";

/// Timex WebSocket client
pub struct TimexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl TimexWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Timex uses format like "ETHBTC" for market IDs
        symbol.replace("/", "")
    }

    fn to_unified_symbol(market_id: &str) -> String {
        let s = market_id.to_uppercase();
        let quotes = ["USDT", "USD", "BTC", "ETH", "EUR", "AUDT"];
        for q in &quotes {
            if s.ends_with(q) {
                let base = &s[..s.len() - q.len()];
                return format!("{base}/{q}");
            }
        }
        s
    }

    fn parse_ticker(data: &TimexWsTicker, symbol: &str) -> Ticker {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: data.vwap,
            open: data.open,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.change,
            percentage: data.percentage,
            average: None,
            base_volume: data.base_volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &TimexWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&e[0]).ok()?,
                        amount: Decimal::from_str(&e[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();
        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&e[0]).ok()?,
                        amount: Decimal::from_str(&e[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();
        OrderBook {
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
            checksum: None,
        }
    }

    fn parse_trade(data: &TimexWsTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn process_message(msg: &str, event_tx: &mpsc::UnboundedSender<WsMessage>) -> CcxtResult<()> {
        let json: serde_json::Value = serde_json::from_str(msg)?;

        // Timex uses Phoenix channel format
        if let Some(event) = json.get("event").and_then(|e| e.as_str()) {
            if let Some(payload) = json.get("payload") {
                let topic = json.get("topic").and_then(|t| t.as_str()).unwrap_or("");
                // Extract market from topic (e.g., "market:ETHBTC")
                let market = topic.split(':').nth(1).unwrap_or("");
                let symbol = Self::to_unified_symbol(market);

                match event {
                    "ticker" | "ticker_update" => {
                        if let Ok(data) = serde_json::from_value::<TimexWsTicker>(payload.clone()) {
                            let ticker = Self::parse_ticker(&data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                                symbol: symbol.clone(),
                                ticker,
                            }));
                        }
                    },
                    "orderbook" | "orderbook_update" | "orderbook_snapshot" => {
                        if let Ok(data) =
                            serde_json::from_value::<TimexWsOrderBook>(payload.clone())
                        {
                            let is_snapshot = event.contains("snapshot");
                            let order_book = Self::parse_order_book(&data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                symbol: symbol.clone(),
                                order_book,
                                is_snapshot,
                            }));
                        }
                    },
                    "trade" | "trades" | "new_trade" => {
                        if let Ok(data) =
                            serde_json::from_value::<Vec<TimexWsTrade>>(payload.clone())
                        {
                            let trades: Vec<Trade> =
                                data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                symbol: symbol.clone(),
                                trades,
                            }));
                        } else if let Ok(data) =
                            serde_json::from_value::<TimexWsTrade>(payload.clone())
                        {
                            let trade = Self::parse_trade(&data, &symbol);
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                symbol: symbol.clone(),
                                trades: vec![trade],
                            }));
                        }
                    },
                    _ => {},
                }
            }
        }

        // Alternative: direct data format
        if let Some(data) = json.get("data") {
            let channel = json.get("channel").and_then(|c| c.as_str()).unwrap_or("");
            let market = json
                .get("market")
                .and_then(|m| m.as_str())
                .or_else(|| data.get("market").and_then(|m| m.as_str()))
                .unwrap_or("");
            let symbol = Self::to_unified_symbol(market);

            if channel.contains("ticker") {
                if let Ok(ticker_data) = serde_json::from_value::<TimexWsTicker>(data.clone()) {
                    let ticker = Self::parse_ticker(&ticker_data, &symbol);
                    let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                        symbol: symbol.clone(),
                        ticker,
                    }));
                }
            } else if channel.contains("orderbook") || channel.contains("depth") {
                if let Ok(book_data) = serde_json::from_value::<TimexWsOrderBook>(data.clone()) {
                    let order_book = Self::parse_order_book(&book_data, &symbol);
                    let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                        symbol: symbol.clone(),
                        order_book,
                        is_snapshot: true,
                    }));
                }
            } else if channel.contains("trade") {
                if let Ok(trades_data) = serde_json::from_value::<Vec<TimexWsTrade>>(data.clone()) {
                    let trades: Vec<Trade> = trades_data
                        .iter()
                        .map(|t| Self::parse_trade(t, &symbol))
                        .collect();
                    let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                        symbol: symbol.clone(),
                        trades,
                    }));
                }
            }
        }

        Ok(())
    }

    async fn subscribe_stream(
        &mut self,
        channel: &str,
        market_id: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });
        let mut ws_rx = ws_client.connect().await?;

        // Timex Phoenix channel subscription format
        let sub_msg = serde_json::json!({
            "topic": format!("market:{}", market_id),
            "event": "phx_join",
            "payload": {
                "channel": channel
            },
            "ref": "1"
        });
        ws_client.send(&sub_msg.to_string())?;

        let mut subs = self.subscriptions.write().await;
        subs.insert(format!("{channel}:{market_id}"), market_id.to_string());
        drop(subs);

        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        let _ = Self::process_message(&msg, &event_tx);
                    },
                    WsEvent::Connected => {
                        let _ = event_tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = event_tx.send(WsMessage::Disconnected);
                        break;
                    },
                    WsEvent::Error(e) => {
                        let _ = event_tx.send(WsMessage::Error(e));
                    },
                    WsEvent::Ping | WsEvent::Pong => {},
                    _ => {},
                }
            }
            let mut subs = subscriptions.write().await;
            subs.clear();
        });
        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }
}

impl Default for TimexWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for TimexWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for TimexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ticker", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("orderbook", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("trades", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_ohlcv(
        &self,
        symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: format!("OHLCV WebSocket for {symbol}"),
        })
    }
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
                ..Default::default()
            });
            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }
    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }
    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct TimexWsTicker {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    vwap: Option<Decimal>,
    #[serde(default)]
    change: Option<Decimal>,
    #[serde(default)]
    percentage: Option<Decimal>,
    #[serde(default, rename = "baseVolume")]
    base_volume: Option<Decimal>,
    #[serde(default, rename = "quoteVolume")]
    quote_volume: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TimexWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TimexWsTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_symbol() {
        assert_eq!(TimexWs::format_symbol("ETH/BTC"), "ETHBTC");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(TimexWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }
    #[test]
    fn test_to_unified_symbol_usdt() {
        assert_eq!(TimexWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
    }
    #[test]
    fn test_default() {
        let ws = TimexWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = TimexWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = TimexWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = TimexWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = TimexWsTicker {
            timestamp: Some(1704067200000),
            high: Some(Decimal::from(50000)),
            low: Some(Decimal::from(48000)),
            bid: Some(Decimal::from(49500)),
            ask: Some(Decimal::from(49600)),
            last: Some(Decimal::from(49550)),
            open: Some(Decimal::from(49000)),
            vwap: None,
            change: None,
            percentage: None,
            base_volume: Some(Decimal::from(100)),
            quote_volume: Some(Decimal::from(4950000)),
        };
        let ticker = TimexWs::parse_ticker(&data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
    }
    #[test]
    fn test_parse_order_book() {
        let data = TimexWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![vec!["49500".into(), "1.5".into()]],
            asks: vec![vec!["49600".into(), "1.0".into()]],
        };
        let ob = TimexWs::parse_order_book(&data, "BTC/USDT");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = TimexWsTrade {
            id: Some("123".into()),
            timestamp: Some(1704067200000),
            price: Some(Decimal::from(49550)),
            amount: Some(Decimal::from(1)),
            side: Some("buy".into()),
        };
        let trade = TimexWs::parse_trade(&data, "BTC/USDT");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"event":"ticker","topic":"market:BTCUSDT","payload":{"last":"49550","timestamp":1704067200000}}"#;
        assert!(TimexWs::process_message(msg, &tx).is_ok());
    }
}
