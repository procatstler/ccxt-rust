//! OceanEx WebSocket Implementation
//!
//! Cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://ws.oceanex.pro/ws/v1/hs";

/// OceanEx WebSocket client
pub struct OceanexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl OceanexWs {
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
        let quotes = ["USDT", "BTC", "ETH", "VET"];
        for q in &quotes {
            if s.ends_with(q) {
                let base = &s[..s.len() - q.len()];
                return format!("{base}/{q}");
            }
        }
        s
    }

    fn parse_ticker(data: &OceanexWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.at.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            high: data.ticker.as_ref().and_then(|t| t.high),
            low: data.ticker.as_ref().and_then(|t| t.low),
            bid: data.ticker.as_ref().and_then(|t| t.buy),
            bid_volume: None,
            ask: data.ticker.as_ref().and_then(|t| t.sell),
            ask_volume: None,
            vwap: None,
            open: data.ticker.as_ref().and_then(|t| t.open),
            close: data.ticker.as_ref().and_then(|t| t.last),
            last: data.ticker.as_ref().and_then(|t| t.last),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.ticker.as_ref().and_then(|t| t.volume),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &OceanexWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data.timestamp.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
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
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            nonce: None,
            bids,
            asks,
        }
    }

    fn parse_trade(data: &OceanexWsTrade, symbol: &str) -> Trade {
        let timestamp = data.created_on.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.volume.unwrap_or(Decimal::ZERO);
        let side = match data.side.as_deref() {
            Some("bid") => Some("buy".to_string()),
            Some("ask") => Some("sell".to_string()),
            _ => None,
        };
        Trade {
            id: data.id.map(|t| t.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
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

        // OceanEx uses event types in messages
        if let Some(event) = json.get("event").and_then(|e| e.as_str()) {
            if let Some(data) = json.get("data") {
                let market = data.get("market").and_then(|m| m.as_str()).unwrap_or("");
                let symbol = Self::to_unified_symbol(market);

                match event {
                    "ticker" => {
                        if let Ok(ticker_data) = serde_json::from_value::<OceanexWsTicker>(data.clone()) {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                        }
                    }
                    "orderbook" | "depth" => {
                        if let Ok(book_data) = serde_json::from_value::<OceanexWsOrderBook>(data.clone()) {
                            let order_book = Self::parse_order_book(&book_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                        }
                    }
                    "trades" | "trade" => {
                        if let Ok(trades_data) = serde_json::from_value::<Vec<OceanexWsTrade>>(data.clone()) {
                            let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
                        } else if let Ok(trade_data) = serde_json::from_value::<OceanexWsTrade>(data.clone()) {
                            let trade = Self::parse_trade(&trade_data, &symbol);
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades: vec![trade] }));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Alternative format: channel-based messages
        if let Some(channel) = json.get("channel").and_then(|c| c.as_str()) {
            if let Some(data) = json.get("data") {
                // Extract market from channel name (e.g., "ticker.btcusdt")
                let parts: Vec<&str> = channel.split('.').collect();
                let market = parts.get(1).unwrap_or(&"");
                let symbol = Self::to_unified_symbol(market);

                if channel.starts_with("ticker") {
                    if let Ok(ticker_data) = serde_json::from_value::<OceanexWsTicker>(data.clone()) {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                    }
                } else if channel.starts_with("orderbook") || channel.starts_with("depth") {
                    if let Ok(book_data) = serde_json::from_value::<OceanexWsOrderBook>(data.clone()) {
                        let order_book = Self::parse_order_book(&book_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                    }
                } else if channel.starts_with("trades") || channel.starts_with("trade") {
                    if let Ok(trades_data) = serde_json::from_value::<Vec<OceanexWsTrade>>(data.clone()) {
                        let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
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

        // OceanEx subscription format
        let sub_msg = serde_json::json!({
            "event": "subscribe",
            "channel": format!("{}.{}", channel, market_id)
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

impl Default for OceanexWs { fn default() -> Self { Self::new() } }
impl Clone for OceanexWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for OceanexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ticker", &Self::format_symbol(symbol)).await
    }
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("orderbook", &Self::format_symbol(symbol)).await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("trades", &Self::format_symbol(symbol)).await
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
struct OceanexWsTickerInfo {
    #[serde(default)] buy: Option<Decimal>,
    #[serde(default)] sell: Option<Decimal>,
    #[serde(default)] low: Option<Decimal>,
    #[serde(default)] high: Option<Decimal>,
    #[serde(default)] last: Option<Decimal>,
    #[serde(default)] open: Option<Decimal>,
    #[serde(default)] volume: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexWsTicker {
    #[serde(default)] at: Option<i64>,
    #[serde(default)] ticker: Option<OceanexWsTickerInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexWsOrderBook {
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] bids: Vec<Vec<String>>,
    #[serde(default)] asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexWsTrade {
    #[serde(default)] id: Option<i64>,
    #[serde(default)] created_on: Option<i64>,
    #[serde(default)] price: Option<Decimal>,
    #[serde(default)] volume: Option<Decimal>,
    #[serde(default)] side: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(OceanexWs::format_symbol("BTC/USDT"), "btcusdt"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(OceanexWs::to_unified_symbol("btcusdt"), "BTC/USDT"); }
    #[test] fn test_default() { let ws = OceanexWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = OceanexWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = OceanexWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = OceanexWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let ticker_info = OceanexWsTickerInfo {
            high: Some(Decimal::from(50000)), low: Some(Decimal::from(48000)),
            buy: Some(Decimal::from(49500)), sell: Some(Decimal::from(49600)),
            last: Some(Decimal::from(49550)), open: Some(Decimal::from(49000)),
            volume: Some(Decimal::from(100))
        };
        let data = OceanexWsTicker { at: Some(1704067200), ticker: Some(ticker_info) };
        let ticker = OceanexWs::parse_ticker(&data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
    }
    #[test]
    fn test_parse_order_book() {
        let data = OceanexWsOrderBook {
            timestamp: Some(1704067200),
            bids: vec![vec!["49500".into(), "1.5".into()]],
            asks: vec![vec!["49600".into(), "1.0".into()]]
        };
        let ob = OceanexWs::parse_order_book(&data, "BTC/USDT");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = OceanexWsTrade {
            id: Some(123), created_on: Some(1704067200),
            price: Some(Decimal::from(49550)), volume: Some(Decimal::from(1)),
            side: Some("bid".into())
        };
        let trade = OceanexWs::parse_trade(&data, "BTC/USDT");
        assert_eq!(trade.id, "123");
        assert_eq!(trade.side, Some("buy".to_string()));
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"event":"ticker","data":{"market":"btcusdt","at":1704067200,"ticker":{"last":"49550"}}}"#;
        assert!(OceanexWs::process_message(msg, &tx).is_ok());
    }
}
