//! Zonda WebSocket Implementation
//!
//! Polish cryptocurrency exchange (formerly BitBay) WebSocket streams

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
    WsExchange, WsMessage, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://api.zondacrypto.exchange/websocket/";

/// Zonda WebSocket client
pub struct ZondaWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl ZondaWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Zonda uses format like "BTC-PLN"
        symbol.replace("/", "-")
    }

    fn to_unified_symbol(market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    fn parse_ticker(data: &ZondaWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            high: data.high_price,
            low: data.low_price,
            bid: data.highest_bid,
            bid_volume: None,
            ask: data.lowest_ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.rate,
            last: data.rate,
            previous_close: data.previous_rate,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &ZondaWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let bids: Vec<OrderBookEntry> = data.buy.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&e.ra).ok()?,
                amount: Decimal::from_str(&e.ca).ok()?,
            })
        }).collect();
        let asks: Vec<OrderBookEntry> = data.sell.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&e.ra).ok()?,
                amount: Decimal::from_str(&e.ca).ok()?,
            })
        }).collect();
        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            nonce: None,
            bids,
            asks,
        }
    }

    fn parse_trade(data: &ZondaWsTrade, symbol: &str) -> Trade {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.rate.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.trade_type.clone(),
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

        if let Some(action) = json.get("action").and_then(|a| a.as_str()) {
            let topic = json.get("topic").and_then(|t| t.as_str()).unwrap_or("");
            // Extract market from topic (e.g., "trading/ticker/BTC-PLN")
            let parts: Vec<&str> = topic.split('/').collect();
            let market = parts.last().unwrap_or(&"");
            let symbol = Self::to_unified_symbol(market);

            if let Some(message) = json.get("message") {
                if action == "push" {
                    if topic.contains("ticker") {
                        if let Ok(ticker_data) = serde_json::from_value::<ZondaWsTicker>(message.clone()) {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                        }
                    } else if topic.contains("orderbook") {
                        if let Ok(book_data) = serde_json::from_value::<ZondaWsOrderBook>(message.clone()) {
                            let order_book = Self::parse_order_book(&book_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                        }
                    } else if topic.contains("trades") {
                        if let Ok(trades_data) = serde_json::from_value::<Vec<ZondaWsTrade>>(message.clone()) {
                            let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
                        } else if let Ok(trade_data) = serde_json::from_value::<ZondaWsTrade>(message.clone()) {
                            let trade = Self::parse_trade(&trade_data, &symbol);
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades: vec![trade] }));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn subscribe_stream(&mut self, channel: &str, market_id: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });
        let mut ws_rx = ws_client.connect().await?;

        // Zonda subscription format
        let sub_msg = serde_json::json!({
            "action": "subscribe-public",
            "module": "trading",
            "path": format!("{}/{}", channel, market_id)
        });
        ws_client.send(&sub_msg.to_string())?;

        let mut subs = self.subscriptions.write().await;
        subs.insert(format!("{channel}:{market_id}"), market_id.to_string());
        drop(subs);

        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => { let _ = Self::process_message(&msg, &event_tx); }
                    WsEvent::Connected => { let _ = event_tx.send(WsMessage::Connected); }
                    WsEvent::Disconnected => { let _ = event_tx.send(WsMessage::Disconnected); break; }
                    WsEvent::Error(e) => { let _ = event_tx.send(WsMessage::Error(e)); }
                    WsEvent::Ping | WsEvent::Pong => {}
                }
            }
            let mut subs = subscriptions.write().await;
            subs.clear();
        });
        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }
}

impl Default for ZondaWs { fn default() -> Self { Self::new() } }
impl Clone for ZondaWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for ZondaWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ticker", &Self::format_symbol(symbol)).await
    }
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("orderbook-limited/10", &Self::format_symbol(symbol)).await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("transactions", &Self::format_symbol(symbol)).await
    }
    async fn watch_ohlcv(&self, symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported { feature: format!("OHLCV WebSocket for {symbol}") })
    }
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_URL.to_string(), auto_reconnect: true, reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10, ping_interval_secs: 30, connect_timeout_secs: 30,
            });
            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }
    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client { ws_client.close()?; self.ws_client = None; }
        Ok(())
    }
    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client { Some(c) => c.is_connected().await, None => false }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaWsTicker {
    #[serde(default)] time: Option<i64>,
    #[serde(default, rename = "highestBid")] highest_bid: Option<Decimal>,
    #[serde(default, rename = "lowestAsk")] lowest_ask: Option<Decimal>,
    #[serde(default)] rate: Option<Decimal>,
    #[serde(default, rename = "previousRate")] previous_rate: Option<Decimal>,
    #[serde(default, rename = "highPrice")] high_price: Option<Decimal>,
    #[serde(default, rename = "lowPrice")] low_price: Option<Decimal>,
    #[serde(default)] volume: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaWsOrderBookEntry {
    #[serde(default)] ra: String, // rate
    #[serde(default)] ca: String, // count/amount
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaWsOrderBook {
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] buy: Vec<ZondaWsOrderBookEntry>,
    #[serde(default)] sell: Vec<ZondaWsOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaWsTrade {
    #[serde(default)] id: Option<String>,
    #[serde(default, rename = "t")] time: Option<i64>,
    #[serde(default, rename = "r")] rate: Option<Decimal>,
    #[serde(default, rename = "a")] amount: Option<Decimal>,
    #[serde(default, rename = "ty")] trade_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(ZondaWs::format_symbol("BTC/PLN"), "BTC-PLN"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(ZondaWs::to_unified_symbol("BTC-PLN"), "BTC/PLN"); }
    #[test] fn test_default() { let ws = ZondaWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = ZondaWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = ZondaWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = ZondaWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let data = ZondaWsTicker {
            time: Some(1704067200000), highest_bid: Some(Decimal::from(180000)),
            lowest_ask: Some(Decimal::from(181000)), rate: Some(Decimal::from(180500)),
            previous_rate: Some(Decimal::from(179000)), high_price: Some(Decimal::from(185000)),
            low_price: Some(Decimal::from(175000)), volume: Some(Decimal::from(100))
        };
        let ticker = ZondaWs::parse_ticker(&data, "BTC/PLN");
        assert_eq!(ticker.symbol, "BTC/PLN");
    }
    #[test]
    fn test_parse_order_book() {
        let data = ZondaWsOrderBook {
            timestamp: Some(1704067200000),
            buy: vec![ZondaWsOrderBookEntry { ra: "180000".into(), ca: "1.5".into() }],
            sell: vec![ZondaWsOrderBookEntry { ra: "181000".into(), ca: "1.0".into() }]
        };
        let ob = ZondaWs::parse_order_book(&data, "BTC/PLN");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = ZondaWsTrade {
            id: Some("123".into()), time: Some(1704067200000),
            rate: Some(Decimal::from(180500)), amount: Some(Decimal::from(1)),
            trade_type: Some("buy".into())
        };
        let trade = ZondaWs::parse_trade(&data, "BTC/PLN");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"action":"push","topic":"trading/ticker/BTC-PLN","message":{"rate":"180500"}}"#;
        assert!(ZondaWs::process_message(msg, &tx).is_ok());
    }
}
