//! Bit2c WebSocket Implementation
//!
//! Israeli cryptocurrency exchange WebSocket streams

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://ws.bit2c.co.il/";

/// Bit2c WebSocket client
pub struct Bit2cWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl Bit2cWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Bit2c uses format like "BtcNis" for BTC/ILS
        symbol.replace("/", "").replace("ILS", "Nis")
    }

    fn to_unified_symbol(market_id: &str) -> String {
        // Convert "BtcNis" to "BTC/ILS"
        let normalized = market_id.replace("Nis", "ILS");
        if normalized.len() >= 6 {
            let base = &normalized[..3];
            let quote = &normalized[3..];
            format!("{}/{}", base.to_uppercase(), quote.to_uppercase())
        } else {
            market_id.to_uppercase()
        }
    }

    fn parse_ticker(data: &Bit2cWsTicker, symbol: &str) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.h,
            low: data.l,
            bid: data.b,
            bid_volume: None,
            ask: data.a,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.ll,
            last: data.ll,
            previous_close: None,
            change: None,
            percentage: None,
            average: data.av,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &Bit2cWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: e.price?,
                amount: e.amount?,
            })
        }).collect();
        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: e.price?,
                amount: e.amount?,
            })
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

    fn parse_trade(data: &Bit2cWsTrade, symbol: &str) -> Trade {
        let timestamp = data.date.map(|d| d * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.tid.map(|t| t.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            symbol: symbol.to_string(),
            trade_type: None,
            side: if data.is_bid.unwrap_or(false) { Some("buy".to_string()) } else { Some("sell".to_string()) },
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

        if let Some(event_type) = json.get("type").and_then(|t| t.as_str()) {
            let pair = json.get("pair").and_then(|p| p.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(pair);

            if let Some(data) = json.get("data") {
                match event_type {
                    "ticker" => {
                        if let Ok(ticker_data) = serde_json::from_value::<Bit2cWsTicker>(data.clone()) {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                        }
                    }
                    "orderbook" | "ob" => {
                        if let Ok(book_data) = serde_json::from_value::<Bit2cWsOrderBook>(data.clone()) {
                            let order_book = Self::parse_order_book(&book_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                        }
                    }
                    "trades" | "trade" => {
                        if let Ok(trades_data) = serde_json::from_value::<Vec<Bit2cWsTrade>>(data.clone()) {
                            let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
                        } else if let Ok(trade_data) = serde_json::from_value::<Bit2cWsTrade>(data.clone()) {
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

impl Default for Bit2cWs { fn default() -> Self { Self::new() } }
impl Clone for Bit2cWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for Bit2cWs {
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
struct Bit2cWsTicker {
    #[serde(default)] h: Option<Decimal>,  // high
    #[serde(default)] l: Option<Decimal>,  // low
    #[serde(default)] ll: Option<Decimal>, // last
    #[serde(default)] b: Option<Decimal>,  // bid
    #[serde(default)] a: Option<Decimal>,  // ask
    #[serde(default)] av: Option<Decimal>, // average
}

#[derive(Debug, Deserialize, Serialize)]
struct Bit2cWsOrderBookEntry {
    #[serde(default)] price: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Bit2cWsOrderBook {
    #[serde(default)] bids: Vec<Bit2cWsOrderBookEntry>,
    #[serde(default)] asks: Vec<Bit2cWsOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Bit2cWsTrade {
    #[serde(default)] tid: Option<i64>,
    #[serde(default)] date: Option<i64>,
    #[serde(default)] price: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
    #[serde(default, rename = "isBid")] is_bid: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(Bit2cWs::format_symbol("BTC/ILS"), "BTCNis"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(Bit2cWs::to_unified_symbol("BtcNis"), "BTC/ILS"); }
    #[test] fn test_default() { let ws = Bit2cWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = Bit2cWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = Bit2cWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = Bit2cWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let data = Bit2cWsTicker {
            h: Some(Decimal::from(150000)), l: Some(Decimal::from(140000)),
            ll: Some(Decimal::from(145000)), b: Some(Decimal::from(144500)),
            a: Some(Decimal::from(145500)), av: Some(Decimal::from(145000))
        };
        let ticker = Bit2cWs::parse_ticker(&data, "BTC/ILS");
        assert_eq!(ticker.symbol, "BTC/ILS");
    }
    #[test]
    fn test_parse_order_book() {
        let data = Bit2cWsOrderBook {
            bids: vec![Bit2cWsOrderBookEntry { price: Some(Decimal::from(144500)), amount: Some(Decimal::from(2)) }],
            asks: vec![Bit2cWsOrderBookEntry { price: Some(Decimal::from(145500)), amount: Some(Decimal::from(1)) }]
        };
        let ob = Bit2cWs::parse_order_book(&data, "BTC/ILS");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = Bit2cWsTrade {
            tid: Some(123), date: Some(1704067200),
            price: Some(Decimal::from(145000)), amount: Some(Decimal::from(1)),
            is_bid: Some(true)
        };
        let trade = Bit2cWs::parse_trade(&data, "BTC/ILS");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"type":"ticker","pair":"BtcNis","data":{"ll":"145000"}}"#;
        assert!(Bit2cWs::process_message(msg, &tx).is_ok());
    }
}
