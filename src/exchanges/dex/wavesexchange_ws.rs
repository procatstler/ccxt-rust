//! Waves Exchange WebSocket Implementation
//!
//! Waves blockchain decentralized exchange WebSocket streams

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://matcher.waves.exchange/ws/v1";

/// Waves Exchange WebSocket client
pub struct WavesexchangeWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl WavesexchangeWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Waves Exchange uses format like "WAVES/USDT"
        symbol.to_uppercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        market_id.to_uppercase()
    }

    fn parse_ticker(data: &WavesWsTicker, symbol: &str) -> Ticker {
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
            vwap: None,
            open: None,
            close: data.last_price,
            last: data.last_price,
            previous_close: None,
            change: data.first_price.zip(data.last_price).map(|(f, l)| l - f),
            percentage: None,
            average: None,
            base_volume: data.volume_waves,
            quote_volume: data.volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &WavesWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|e| {
                Some(OrderBookEntry {
                    price: e.price?,
                    amount: e.amount?,
                })
            })
            .collect();
        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|e| {
                Some(OrderBookEntry {
                    price: e.price?,
                    amount: e.amount?,
                })
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

    fn parse_trade(data: &WavesWsTrade, symbol: &str) -> Trade {
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
            side: data.order_type.clone(),
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

        if let Some(msg_type) = json.get("T").and_then(|t| t.as_str()) {
            let symbol = json
                .get("S")
                .and_then(|s| s.as_str())
                .map(Self::to_unified_symbol)
                .unwrap_or_default();

            match msg_type {
                "q" => {
                    // Ticker/Quote update
                    if let Ok(ticker_data) = serde_json::from_value::<WavesWsTicker>(json.clone()) {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                            symbol: symbol.clone(),
                            ticker,
                        }));
                    }
                },
                "ob" | "obs" => {
                    // Order book snapshot
                    if let Ok(book_data) = serde_json::from_value::<WavesWsOrderBook>(json.clone())
                    {
                        let order_book = Self::parse_order_book(&book_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                            symbol: symbol.clone(),
                            order_book,
                            is_snapshot: true,
                        }));
                    }
                },
                "t" => {
                    // Trade update
                    if let Some(trades) = json.get("o") {
                        if let Ok(trades_data) =
                            serde_json::from_value::<Vec<WavesWsTrade>>(trades.clone())
                        {
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
                },
                _ => {},
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

        let sub_msg = serde_json::json!({
            "T": "subscribe",
            "S": market_id,
            "t": channel
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

impl Default for WavesexchangeWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for WavesexchangeWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for WavesexchangeWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("q", &Self::format_symbol(symbol)).await
    }
    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ob", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("t", &Self::format_symbol(symbol)).await
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
struct WavesWsTicker {
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
    #[serde(default, rename = "lp")]
    last_price: Option<Decimal>,
    #[serde(default, rename = "fp")]
    first_price: Option<Decimal>,
    #[serde(default, rename = "v")]
    volume: Option<Decimal>,
    #[serde(default, rename = "vw")]
    volume_waves: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WavesWsOrderEntry {
    #[serde(default, rename = "p")]
    price: Option<Decimal>,
    #[serde(default, rename = "a")]
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WavesWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "b")]
    bids: Vec<WavesWsOrderEntry>,
    #[serde(default, rename = "a")]
    asks: Vec<WavesWsOrderEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WavesWsTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "p")]
    price: Option<Decimal>,
    #[serde(default, rename = "a")]
    amount: Option<Decimal>,
    #[serde(default, rename = "ot")]
    order_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_symbol() {
        assert_eq!(WavesexchangeWs::format_symbol("WAVES/USDT"), "WAVES/USDT");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(
            WavesexchangeWs::to_unified_symbol("WAVES/USDT"),
            "WAVES/USDT"
        );
    }
    #[test]
    fn test_default() {
        let ws = WavesexchangeWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = WavesexchangeWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = WavesexchangeWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = WavesexchangeWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = WavesWsTicker {
            timestamp: Some(1704067200000),
            high: Some(Decimal::from(5)),
            low: Some(Decimal::from(4)),
            bid: Some(Decimal::from(4)),
            ask: Some(Decimal::from(5)),
            last_price: Some(Decimal::from(4)),
            first_price: Some(Decimal::from(4)),
            volume: Some(Decimal::from(1000000)),
            volume_waves: Some(Decimal::from(250000)),
        };
        let ticker = WavesexchangeWs::parse_ticker(&data, "WAVES/USDT");
        assert_eq!(ticker.symbol, "WAVES/USDT");
    }
    #[test]
    fn test_parse_order_book() {
        let data = WavesWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![WavesWsOrderEntry {
                price: Some(Decimal::from(4)),
                amount: Some(Decimal::from(100)),
            }],
            asks: vec![WavesWsOrderEntry {
                price: Some(Decimal::from(5)),
                amount: Some(Decimal::from(50)),
            }],
        };
        let ob = WavesexchangeWs::parse_order_book(&data, "WAVES/USDT");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = WavesWsTrade {
            id: Some("abc123".into()),
            timestamp: Some(1704067200000),
            price: Some(Decimal::from(4)),
            amount: Some(Decimal::from(100)),
            order_type: Some("buy".into()),
        };
        let trade = WavesexchangeWs::parse_trade(&data, "WAVES/USDT");
        assert_eq!(trade.id, "abc123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"T":"q","S":"WAVES/USDT","lp":"4.5"}"#;
        assert!(WavesexchangeWs::process_message(msg, &tx).is_ok());
    }
}
