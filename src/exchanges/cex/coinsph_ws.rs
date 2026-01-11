//! Coins.ph WebSocket Implementation
//!
//! Philippine cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://api.pro.coins.ph/ws";

/// Coins.ph WebSocket client
pub struct CoinsphWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl CoinsphWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Coins.ph uses format like "BTCPHP"
        symbol.replace("/", "").to_uppercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        // Convert "BTCPHP" to "BTC/PHP"
        if market_id.len() >= 6 {
            let base = &market_id[..3];
            let quote = &market_id[3..];
            format!("{base}/{quote}")
        } else {
            market_id.to_string()
        }
    }

    fn parse_ticker(data: &CoinsphWsTicker, symbol: &str) -> Ticker {
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
            close: data.close,
            last: data.close,
            previous_close: None,
            change: None,
            percentage: data.change_percent,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &CoinsphWsOrderBook, symbol: &str) -> OrderBook {
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

    fn parse_trade(data: &CoinsphWsTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.quantity.unwrap_or(Decimal::ZERO);
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
            side: if data.is_buyer_maker.unwrap_or(false) {
                Some("sell".to_string())
            } else {
                Some("buy".to_string())
            },
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

        if let Some(stream) = json.get("stream").and_then(|s| s.as_str()) {
            // Stream format: "btcphp@ticker"
            let parts: Vec<&str> = stream.split('@').collect();
            let market = parts.first().unwrap_or(&"");
            let channel = parts.get(1).unwrap_or(&"");
            let symbol = Self::to_unified_symbol(&market.to_uppercase());

            if let Some(data) = json.get("data") {
                if channel.contains("ticker") {
                    if let Ok(ticker_data) = serde_json::from_value::<CoinsphWsTicker>(data.clone())
                    {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                            symbol: symbol.clone(),
                            ticker,
                        }));
                    }
                } else if channel.contains("depth") {
                    if let Ok(book_data) =
                        serde_json::from_value::<CoinsphWsOrderBook>(data.clone())
                    {
                        let order_book = Self::parse_order_book(&book_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                            symbol: symbol.clone(),
                            order_book,
                            is_snapshot: true,
                        }));
                    }
                } else if channel.contains("trade") {
                    if let Ok(trade_data) = serde_json::from_value::<CoinsphWsTrade>(data.clone()) {
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

        let stream = format!("{}@{}", market_id.to_lowercase(), channel);
        let sub_msg = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": [stream]
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

impl Default for CoinsphWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for CoinsphWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for CoinsphWs {
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
struct CoinsphWsTicker {
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
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default, rename = "quoteVolume")]
    quote_volume: Option<Decimal>,
    #[serde(default, rename = "changePercent")]
    change_percent: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinsphWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinsphWsTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
    #[serde(default, rename = "isBuyerMaker")]
    is_buyer_maker: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinsphWs::format_symbol("BTC/PHP"), "BTCPHP");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinsphWs::to_unified_symbol("BTCPHP"), "BTC/PHP");
    }
    #[test]
    fn test_default() {
        let ws = CoinsphWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = CoinsphWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = CoinsphWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = CoinsphWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = CoinsphWsTicker {
            timestamp: Some(1704067200000),
            high: Some(Decimal::from(3000000)),
            low: Some(Decimal::from(2800000)),
            bid: Some(Decimal::from(2950000)),
            ask: Some(Decimal::from(2960000)),
            open: Some(Decimal::from(2900000)),
            close: Some(Decimal::from(2955000)),
            volume: Some(Decimal::from(100)),
            quote_volume: None,
            change_percent: None,
        };
        let ticker = CoinsphWs::parse_ticker(&data, "BTC/PHP");
        assert_eq!(ticker.symbol, "BTC/PHP");
    }
    #[test]
    fn test_parse_order_book() {
        let data = CoinsphWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![vec!["2950000".into(), "1.5".into()]],
            asks: vec![vec!["2960000".into(), "1.0".into()]],
        };
        let ob = CoinsphWs::parse_order_book(&data, "BTC/PHP");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = CoinsphWsTrade {
            id: Some("123".into()),
            timestamp: Some(1704067200000),
            price: Some(Decimal::from(2955000)),
            quantity: Some(Decimal::from(1)),
            is_buyer_maker: Some(false),
        };
        let trade = CoinsphWs::parse_trade(&data, "BTC/PHP");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"stream":"btcphp@ticker","data":{"close":"2955000"}}"#;
        assert!(CoinsphWs::process_message(msg, &tx).is_ok());
    }
}
