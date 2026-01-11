//! BitoPro WebSocket Implementation
//!
//! Taiwanese cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://stream.bitopro.com:9443/ws/v1/pub";

/// BitoPro WebSocket client
pub struct BitoproWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BitoproWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // BitoPro uses format like "btc_twd" for BTC/TWD
        symbol.replace("/", "_").to_lowercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        market_id.to_uppercase().replace("_", "/")
    }

    fn parse_ticker(data: &BitoproWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            high: data.high_24hr,
            low: data.low_24hr,
            bid: data.highest_bid,
            bid_volume: None,
            ask: data.lowest_ask,
            ask_volume: None,
            vwap: None,
            open: data.open,
            close: data.last_price,
            last: data.last_price,
            previous_close: None,
            change: data.price_change_24hr,
            percentage: None,
            average: None,
            base_volume: data.volume_24hr,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &BitoproWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
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

    fn parse_trade(data: &BitoproWsTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            symbol: symbol.to_string(),
            trade_type: None,
            side: if data.is_buy.unwrap_or(false) { Some("buy".to_string()) } else { Some("sell".to_string()) },
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

        if let Some(event_type) = json.get("event").and_then(|e| e.as_str()) {
            let pair = json.get("pair").and_then(|p| p.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(pair);

            if let Some(data) = json.get("data") {
                match event_type {
                    "TICKER" | "ticker" => {
                        if let Ok(ticker_data) = serde_json::from_value::<BitoproWsTicker>(data.clone()) {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                        }
                    }
                    "ORDER_BOOK" | "orderbook" => {
                        if let Ok(book_data) = serde_json::from_value::<BitoproWsOrderBook>(data.clone()) {
                            let order_book = Self::parse_order_book(&book_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                        }
                    }
                    "TRADE" | "trade" => {
                        if let Ok(trades_data) = serde_json::from_value::<Vec<BitoproWsTrade>>(data.clone()) {
                            let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
                        } else if let Ok(trade_data) = serde_json::from_value::<BitoproWsTrade>(data.clone()) {
                            let trade = Self::parse_trade(&trade_data, &symbol);
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades: vec![trade] }));
                        }
                    }
                    _ => {}
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

        let sub_msg = serde_json::json!({
            "action": "subscribe",
            "type": channel,
            "pair": market_id
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

impl Default for BitoproWs { fn default() -> Self { Self::new() } }
impl Clone for BitoproWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for BitoproWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("TICKER", &Self::format_symbol(symbol)).await
    }
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ORDER_BOOK", &Self::format_symbol(symbol)).await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("TRADE", &Self::format_symbol(symbol)).await
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
struct BitoproWsTicker {
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default, rename = "high24hr")] high_24hr: Option<Decimal>,
    #[serde(default, rename = "low24hr")] low_24hr: Option<Decimal>,
    #[serde(default, rename = "highestBid")] highest_bid: Option<Decimal>,
    #[serde(default, rename = "lowestAsk")] lowest_ask: Option<Decimal>,
    #[serde(default, rename = "lastPrice")] last_price: Option<Decimal>,
    #[serde(default)] open: Option<Decimal>,
    #[serde(default, rename = "priceChange24hr")] price_change_24hr: Option<Decimal>,
    #[serde(default, rename = "volume24hr")] volume_24hr: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitoproWsOrderBook {
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] bids: Vec<Vec<String>>,
    #[serde(default)] asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitoproWsTrade {
    #[serde(default, rename = "tradeId")] trade_id: Option<String>,
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] price: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
    #[serde(default, rename = "isBuy")] is_buy: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(BitoproWs::format_symbol("BTC/TWD"), "btc_twd"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(BitoproWs::to_unified_symbol("btc_twd"), "BTC/TWD"); }
    #[test] fn test_default() { let ws = BitoproWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = BitoproWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = BitoproWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = BitoproWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let data = BitoproWsTicker {
            timestamp: Some(1704067200000), high_24hr: Some(Decimal::from(1200000)),
            low_24hr: Some(Decimal::from(1100000)), highest_bid: Some(Decimal::from(1150000)),
            lowest_ask: Some(Decimal::from(1155000)), last_price: Some(Decimal::from(1152000)),
            open: Some(Decimal::from(1140000)), price_change_24hr: None, volume_24hr: Some(Decimal::from(500))
        };
        let ticker = BitoproWs::parse_ticker(&data, "BTC/TWD");
        assert_eq!(ticker.symbol, "BTC/TWD");
    }
    #[test]
    fn test_parse_order_book() {
        let data = BitoproWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![vec!["1150000".into(), "2.0".into()]],
            asks: vec![vec!["1155000".into(), "1.5".into()]]
        };
        let ob = BitoproWs::parse_order_book(&data, "BTC/TWD");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = BitoproWsTrade {
            trade_id: Some("123".into()), timestamp: Some(1704067200000),
            price: Some(Decimal::from(1152000)), amount: Some(Decimal::from(1)),
            is_buy: Some(true)
        };
        let trade = BitoproWs::parse_trade(&data, "BTC/TWD");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"event":"TICKER","pair":"btc_twd","data":{"lastPrice":"1152000"}}"#;
        assert!(BitoproWs::process_message(msg, &tx).is_ok());
    }
}
