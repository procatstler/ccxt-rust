//! BTC-Alpha WebSocket Implementation
//!
//! International cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://btc-alpha.com/ws/";

/// BTC-Alpha WebSocket client
pub struct BtcalphaWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BtcalphaWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // BTC-Alpha uses format like "BTC_USDT"
        symbol.replace("/", "_").to_uppercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        market_id.replace("_", "/")
    }

    fn parse_ticker(data: &BtcalphaWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
            high: data.high,
            low: data.low,
            bid: data.buy,
            bid_volume: None,
            ask: data.sell,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &BtcalphaWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();
        let bids: Vec<OrderBookEntry> = data.buy.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: e.price?,
                amount: e.amount?,
            })
        }).collect();
        let asks: Vec<OrderBookEntry> = data.sell.iter().filter_map(|e| {
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

    fn parse_trade(data: &BtcalphaWsTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()).unwrap_or_default()),
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
            let pair = json.get("pair").and_then(|p| p.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(pair);

            if let Some(data) = json.get("data") {
                if channel.contains("ticker") {
                    if let Ok(ticker_data) = serde_json::from_value::<BtcalphaWsTicker>(data.clone()) {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                    }
                } else if channel.contains("orderbook") || channel.contains("depth") {
                    if let Ok(book_data) = serde_json::from_value::<BtcalphaWsOrderBook>(data.clone()) {
                        let order_book = Self::parse_order_book(&book_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                    }
                } else if channel.contains("trade") {
                    if let Ok(trades_data) = serde_json::from_value::<Vec<BtcalphaWsTrade>>(data.clone()) {
                        let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
                    } else if let Ok(trade_data) = serde_json::from_value::<BtcalphaWsTrade>(data.clone()) {
                        let trade = Self::parse_trade(&trade_data, &symbol);
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades: vec![trade] }));
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

        let sub_msg = serde_json::json!({
            "action": "subscribe",
            "channel": channel,
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

impl Default for BtcalphaWs { fn default() -> Self { Self::new() } }
impl Clone for BtcalphaWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for BtcalphaWs {
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
struct BtcalphaWsTicker {
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] high: Option<Decimal>,
    #[serde(default)] low: Option<Decimal>,
    #[serde(default)] buy: Option<Decimal>,
    #[serde(default)] sell: Option<Decimal>,
    #[serde(default)] last: Option<Decimal>,
    #[serde(default)] vol: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaWsOrderEntry {
    #[serde(default)] price: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaWsOrderBook {
    #[serde(default)] buy: Vec<BtcalphaWsOrderEntry>,
    #[serde(default)] sell: Vec<BtcalphaWsOrderEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaWsTrade {
    #[serde(default)] id: Option<i64>,
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] price: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
    #[serde(default)] side: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(BtcalphaWs::format_symbol("BTC/USDT"), "BTC_USDT"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(BtcalphaWs::to_unified_symbol("BTC_USDT"), "BTC/USDT"); }
    #[test] fn test_default() { let ws = BtcalphaWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = BtcalphaWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = BtcalphaWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = BtcalphaWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let data = BtcalphaWsTicker {
            timestamp: Some(1704067200000), high: Some(Decimal::from(45000)),
            low: Some(Decimal::from(43000)), buy: Some(Decimal::from(44500)),
            sell: Some(Decimal::from(44600)), last: Some(Decimal::from(44550)),
            vol: Some(Decimal::from(100))
        };
        let ticker = BtcalphaWs::parse_ticker(&data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
    }
    #[test]
    fn test_parse_order_book() {
        let data = BtcalphaWsOrderBook {
            buy: vec![BtcalphaWsOrderEntry { price: Some(Decimal::from(44500)), amount: Some(Decimal::from(2)) }],
            sell: vec![BtcalphaWsOrderEntry { price: Some(Decimal::from(44600)), amount: Some(Decimal::from(1)) }]
        };
        let ob = BtcalphaWs::parse_order_book(&data, "BTC/USDT");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = BtcalphaWsTrade {
            id: Some(123), timestamp: Some(1704067200000),
            price: Some(Decimal::from(44550)), amount: Some(Decimal::from(1)),
            side: Some("buy".into())
        };
        let trade = BtcalphaWs::parse_trade(&data, "BTC/USDT");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"ticker","pair":"BTC_USDT","data":{"last":"44550"}}"#;
        assert!(BtcalphaWs::process_message(msg, &tx).is_ok());
    }
}
