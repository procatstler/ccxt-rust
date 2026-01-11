//! BTCTurk WebSocket Implementation
//!
//! Turkish cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://ws-feed-pro.btcturk.com/";

/// BTCTurk WebSocket client
pub struct BtcturkWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BtcturkWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // BTCTurk uses format like "BTCTRY"
        symbol.replace("/", "").to_uppercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        // Convert "BTCTRY" to "BTC/TRY"
        if market_id.len() >= 6 {
            let base = &market_id[..3];
            let quote = &market_id[3..];
            format!("{base}/{quote}")
        } else {
            market_id.to_string()
        }
    }

    fn parse_ticker(data: &BtcturkWsTicker, symbol: &str) -> Ticker {
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
            open: data.open,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.daily_change,
            percentage: data.daily_change_percent,
            average: data.average,
            base_volume: data.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &BtcturkWsOrderBook, symbol: &str) -> OrderBook {
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

    fn parse_trade(data: &BtcturkWsTrade, symbol: &str) -> Trade {
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

        if let Some(channel) = json.get("channel").and_then(|c| c.as_str()) {
            let pair = json
                .get("pairSymbol")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let symbol = Self::to_unified_symbol(pair);

            if let Some(data) = json.get("data") {
                if channel.contains("ticker") {
                    if let Ok(ticker_data) = serde_json::from_value::<BtcturkWsTicker>(data.clone())
                    {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                            symbol: symbol.clone(),
                            ticker,
                        }));
                    }
                } else if channel.contains("orderbook") {
                    if let Ok(book_data) =
                        serde_json::from_value::<BtcturkWsOrderBook>(data.clone())
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
                        serde_json::from_value::<Vec<BtcturkWsTrade>>(data.clone())
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
                        serde_json::from_value::<BtcturkWsTrade>(data.clone())
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

        let sub_msg = serde_json::json!([151, {
            "type": 151,
            "channel": channel,
            "event": market_id,
            "join": true
        }]);
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

impl Default for BtcturkWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for BtcturkWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for BtcturkWs {
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
        ws.subscribe_stream("trade", &Self::format_symbol(symbol))
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
struct BtcturkWsTicker {
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
    volume: Option<Decimal>,
    #[serde(default)]
    average: Option<Decimal>,
    #[serde(default, rename = "dailyChange")]
    daily_change: Option<Decimal>,
    #[serde(default, rename = "dailyChangePercent")]
    daily_change_percent: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcturkWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcturkWsTrade {
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
        assert_eq!(BtcturkWs::format_symbol("BTC/TRY"), "BTCTRY");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BtcturkWs::to_unified_symbol("BTCTRY"), "BTC/TRY");
    }
    #[test]
    fn test_default() {
        let ws = BtcturkWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = BtcturkWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = BtcturkWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = BtcturkWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = BtcturkWsTicker {
            timestamp: Some(1704067200000),
            high: Some(Decimal::from(1500000)),
            low: Some(Decimal::from(1400000)),
            bid: Some(Decimal::from(1450000)),
            ask: Some(Decimal::from(1455000)),
            last: Some(Decimal::from(1452000)),
            open: Some(Decimal::from(1440000)),
            volume: Some(Decimal::from(100)),
            average: None,
            daily_change: None,
            daily_change_percent: None,
        };
        let ticker = BtcturkWs::parse_ticker(&data, "BTC/TRY");
        assert_eq!(ticker.symbol, "BTC/TRY");
    }
    #[test]
    fn test_parse_order_book() {
        let data = BtcturkWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![vec!["1450000".into(), "1.5".into()]],
            asks: vec![vec!["1455000".into(), "1.0".into()]],
        };
        let ob = BtcturkWs::parse_order_book(&data, "BTC/TRY");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = BtcturkWsTrade {
            id: Some("123".into()),
            timestamp: Some(1704067200000),
            price: Some(Decimal::from(1452000)),
            amount: Some(Decimal::from(1)),
            side: Some("buy".into()),
        };
        let trade = BtcturkWs::parse_trade(&data, "BTC/TRY");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"ticker","pairSymbol":"BTCTRY","data":{"last":"1452000"}}"#;
        assert!(BtcturkWs::process_message(msg, &tx).is_ok());
    }
}
