//! Tokocrypto WebSocket Implementation
//!
//! Indonesian cryptocurrency exchange WebSocket streams (Binance partner)

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

const WS_URL: &str = "wss://stream.tokocrypto.com/stream";

/// Tokocrypto WebSocket client
pub struct TokocryptoWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl TokocryptoWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        let s = market_id.to_uppercase();
        let quotes = ["USDT", "BIDR", "BTC", "ETH"];
        for q in &quotes {
            if s.ends_with(q) {
                let base = &s[..s.len() - q.len()];
                return format!("{base}/{q}");
            }
        }
        s
    }

    fn parse_ticker(data: &TokocryptoWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.event_time.unwrap_or_else(|| Utc::now().timestamp_millis());
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            high: data.high,
            low: data.low,
            bid: data.bid,
            bid_volume: data.bid_qty,
            ask: data.ask,
            ask_volume: data.ask_qty,
            vwap: None,
            open: data.open,
            close: data.close,
            last: data.close,
            previous_close: None,
            change: data.price_change,
            percentage: data.price_change_percent,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &TokocryptoWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|e| {
            if e.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&e[0]).ok()?,
                    amount: Decimal::from_str(&e[1]).ok()?,
                })
            } else { None }
        }).collect();
        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|e| {
            if e.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&e[0]).ok()?,
                    amount: Decimal::from_str(&e[1]).ok()?,
                })
            } else { None }
        }).collect();
        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        }
    }

    fn parse_trade(data: &TokocryptoWsTrade, symbol: &str) -> Trade {
        let timestamp = data.trade_time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.quantity.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.trade_id.map(|t| t.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.is_buyer_maker.map(|m| if m { "sell" } else { "buy" }.to_string()),
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
        if let Some(event) = json.get("e").and_then(|e| e.as_str()) {
            let market = json.get("s").and_then(|s| s.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(market);
            match event {
                "24hrTicker" => {
                    if let Ok(data) = serde_json::from_value::<TokocryptoWsTicker>(json.clone()) {
                        let ticker = Self::parse_ticker(&data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                    }
                }
                "depthUpdate" => {
                    if let Ok(data) = serde_json::from_value::<TokocryptoWsOrderBook>(json.clone()) {
                        let order_book = Self::parse_order_book(&data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: false }));
                    }
                }
                "trade" => {
                    if let Ok(data) = serde_json::from_value::<TokocryptoWsTrade>(json.clone()) {
                        let trade = Self::parse_trade(&data, &symbol);
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades: vec![trade] }));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn subscribe_stream(&mut self, channel: &str, market_id: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());
        let stream = match channel {
            "ticker" => format!("{market_id}@ticker"),
            "depth" => format!("{market_id}@depth20"),
            "trade" => format!("{market_id}@trade"),
            _ => return Err(crate::errors::CcxtError::NotSupported { feature: channel.to_string() }),
        };
        let ws_url = format!("{WS_URL}?streams={stream}");
        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });
        let mut ws_rx = ws_client.connect().await?;
        let mut subs = self.subscriptions.write().await;
        subs.insert(format!("{channel}:{market_id}"), market_id.to_string());
        drop(subs);
        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(&msg) {
                            if let Some(data) = wrapper.get("data") {
                                let _ = Self::process_message(&data.to_string(), &event_tx);
                            }
                        }
                    }
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

impl Default for TokocryptoWs { fn default() -> Self { Self::new() } }
impl Clone for TokocryptoWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for TokocryptoWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ticker", &Self::format_symbol(symbol)).await
    }
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("depth", &Self::format_symbol(symbol)).await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("trade", &Self::format_symbol(symbol)).await
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
struct TokocryptoWsTicker {
    #[serde(default, rename = "E")] event_time: Option<i64>,
    #[serde(default, rename = "h")] high: Option<Decimal>,
    #[serde(default, rename = "l")] low: Option<Decimal>,
    #[serde(default, rename = "o")] open: Option<Decimal>,
    #[serde(default, rename = "c")] close: Option<Decimal>,
    #[serde(default, rename = "b")] bid: Option<Decimal>,
    #[serde(default, rename = "B")] bid_qty: Option<Decimal>,
    #[serde(default, rename = "a")] ask: Option<Decimal>,
    #[serde(default, rename = "A")] ask_qty: Option<Decimal>,
    #[serde(default, rename = "v")] volume: Option<Decimal>,
    #[serde(default, rename = "q")] quote_volume: Option<Decimal>,
    #[serde(default, rename = "p")] price_change: Option<Decimal>,
    #[serde(default, rename = "P")] price_change_percent: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokocryptoWsOrderBook {
    #[serde(default, rename = "b")] bids: Vec<Vec<String>>,
    #[serde(default, rename = "a")] asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokocryptoWsTrade {
    #[serde(default, rename = "t")] trade_id: Option<i64>,
    #[serde(default, rename = "T")] trade_time: Option<i64>,
    #[serde(default, rename = "p")] price: Option<Decimal>,
    #[serde(default, rename = "q")] quantity: Option<Decimal>,
    #[serde(default, rename = "m")] is_buyer_maker: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(TokocryptoWs::format_symbol("BTC/USDT"), "btcusdt"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(TokocryptoWs::to_unified_symbol("BTCUSDT"), "BTC/USDT"); }
    #[test] fn test_default() { let ws = TokocryptoWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = TokocryptoWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = TokocryptoWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = TokocryptoWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let data = TokocryptoWsTicker { event_time: Some(1704067200000), high: Some(Decimal::from(50000)),
            low: Some(Decimal::from(48000)), open: Some(Decimal::from(49000)), close: Some(Decimal::from(49550)),
            bid: Some(Decimal::from(49500)), bid_qty: Some(Decimal::from(1)), ask: Some(Decimal::from(49600)),
            ask_qty: Some(Decimal::from(2)), volume: Some(Decimal::from(100)), quote_volume: Some(Decimal::from(4950000)),
            price_change: Some(Decimal::from(550)), price_change_percent: Some(Decimal::from(1)) };
        let ticker = TokocryptoWs::parse_ticker(&data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
    }
    #[test]
    fn test_parse_order_book() {
        let data = TokocryptoWsOrderBook { bids: vec![vec!["49500".into(), "1.5".into()]], asks: vec![vec!["49600".into(), "1.0".into()]] };
        let ob = TokocryptoWs::parse_order_book(&data, "BTC/USDT");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = TokocryptoWsTrade { trade_id: Some(123), trade_time: Some(1704067200000),
            price: Some(Decimal::from(49550)), quantity: Some(Decimal::from(1)), is_buyer_maker: Some(false) };
        let trade = TokocryptoWs::parse_trade(&data, "BTC/USDT");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"e":"24hrTicker","s":"BTCUSDT","c":"49550"}"#;
        assert!(TokocryptoWs::process_message(msg, &tx).is_ok());
    }
}
