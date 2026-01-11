//! Paymium WebSocket Implementation
//!
//! French cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://paymium.com/ws/public";

/// Paymium WebSocket client
pub struct PaymiumWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl PaymiumWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Paymium uses format like "btceur"
        symbol.replace("/", "").to_lowercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        // Convert "btceur" to "BTC/EUR"
        let upper = market_id.to_uppercase();
        if upper.len() >= 6 {
            let base = &upper[..3];
            let quote = &upper[3..];
            format!("{base}/{quote}")
        } else {
            upper
        }
    }

    fn parse_ticker(data: &PaymiumWsTicker, symbol: &str) -> Ticker {
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
            open: None,
            close: data.price,
            last: data.price,
            previous_close: None,
            change: None,
            percentage: data.variation,
            average: None,
            base_volume: data.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &PaymiumWsOrderBook, symbol: &str) -> OrderBook {
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

    fn parse_trade(data: &PaymiumWsTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.uuid.clone().unwrap_or_default(),
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

        if let Some(channel) = json.get("channel").and_then(|c| c.as_str()) {
            let currency = json
                .get("currency")
                .and_then(|p| p.as_str())
                .unwrap_or("btceur");
            let symbol = Self::to_unified_symbol(currency);

            if let Some(data) = json.get("data") {
                if channel.contains("ticker") {
                    if let Ok(ticker_data) = serde_json::from_value::<PaymiumWsTicker>(data.clone())
                    {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                            symbol: symbol.clone(),
                            ticker,
                        }));
                    }
                } else if channel.contains("depth") || channel.contains("orderbook") {
                    if let Ok(book_data) =
                        serde_json::from_value::<PaymiumWsOrderBook>(data.clone())
                    {
                        let order_book = Self::parse_order_book(&book_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                            symbol: symbol.clone(),
                            order_book,
                            is_snapshot: true,
                        }));
                    }
                } else if channel.contains("trade") {
                    if let Ok(trades_data) =
                        serde_json::from_value::<Vec<PaymiumWsTrade>>(data.clone())
                    {
                        let trades: Vec<Trade> = trades_data
                            .iter()
                            .map(|t| Self::parse_trade(t, &symbol))
                            .collect();
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                            symbol: symbol.clone(),
                            trades,
                        }));
                    } else if let Ok(trade_data) =
                        serde_json::from_value::<PaymiumWsTrade>(data.clone())
                    {
                        let trade = Self::parse_trade(&trade_data, &symbol);
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                            symbol: symbol.clone(),
                            trades: vec![trade],
                        }));
                    }
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

        let sub_msg = serde_json::json!({
            "op": "subscribe",
            "channel": channel,
            "currency": market_id
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

impl Default for PaymiumWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for PaymiumWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for PaymiumWs {
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
        ws.subscribe_stream("depth", &Self::format_symbol(symbol))
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
struct PaymiumWsTicker {
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
    price: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    vwap: Option<Decimal>,
    #[serde(default)]
    variation: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumWsTrade {
    #[serde(default)]
    uuid: Option<String>,
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
        assert_eq!(PaymiumWs::format_symbol("BTC/EUR"), "btceur");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(PaymiumWs::to_unified_symbol("btceur"), "BTC/EUR");
    }
    #[test]
    fn test_default() {
        let ws = PaymiumWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = PaymiumWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = PaymiumWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = PaymiumWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = PaymiumWsTicker {
            timestamp: Some(1704067200000),
            high: Some(Decimal::from(40000)),
            low: Some(Decimal::from(38000)),
            bid: Some(Decimal::from(39500)),
            ask: Some(Decimal::from(39600)),
            price: Some(Decimal::from(39550)),
            volume: Some(Decimal::from(100)),
            vwap: None,
            variation: None,
        };
        let ticker = PaymiumWs::parse_ticker(&data, "BTC/EUR");
        assert_eq!(ticker.symbol, "BTC/EUR");
    }
    #[test]
    fn test_parse_order_book() {
        let data = PaymiumWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![vec!["39500".into(), "1.5".into()]],
            asks: vec![vec!["39600".into(), "1.0".into()]],
        };
        let ob = PaymiumWs::parse_order_book(&data, "BTC/EUR");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = PaymiumWsTrade {
            uuid: Some("abc123".into()),
            timestamp: Some(1704067200000),
            price: Some(Decimal::from(39550)),
            amount: Some(Decimal::from(1)),
            side: Some("buy".into()),
        };
        let trade = PaymiumWs::parse_trade(&data, "BTC/EUR");
        assert_eq!(trade.id, "abc123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"ticker","currency":"btceur","data":{"price":"39550"}}"#;
        assert!(PaymiumWs::process_message(msg, &tx).is_ok());
    }
}
