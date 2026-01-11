//! Coinspot WebSocket Implementation
//!
//! Australian cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://ws.coinspot.com.au/pubsub";

/// Coinspot WebSocket client
pub struct CoinspotWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl CoinspotWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Coinspot uses format like "BTC" for BTC/AUD
        symbol.split('/').next().unwrap_or("BTC").to_uppercase()
    }

    fn to_unified_symbol(coin: &str) -> String {
        format!("{}/AUD", coin.to_uppercase())
    }

    fn parse_ticker(data: &CoinspotWsTicker, symbol: &str) -> Ticker {
        let timestamp = Utc::now().timestamp_millis();
        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: None,
            low: None,
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &CoinspotWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();
        let bids: Vec<OrderBookEntry> = data.buyorders.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: e.rate?,
                amount: e.amount?,
            })
        }).collect();
        let asks: Vec<OrderBookEntry> = data.sellorders.iter().filter_map(|e| {
            Some(OrderBookEntry {
                price: e.rate?,
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

    fn parse_trade(data: &CoinspotWsTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
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
            side: data.market.clone(),
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
            let coin = json.get("coin").and_then(|c| c.as_str()).unwrap_or("BTC");
            let symbol = Self::to_unified_symbol(coin);

            if let Some(data) = json.get("data") {
                if channel.contains("ticker") {
                    if let Ok(ticker_data) = serde_json::from_value::<CoinspotWsTicker>(data.clone()) {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent { symbol: symbol.clone(), ticker }));
                    }
                } else if channel.contains("orders") || channel.contains("orderbook") {
                    if let Ok(book_data) = serde_json::from_value::<CoinspotWsOrderBook>(data.clone()) {
                        let order_book = Self::parse_order_book(&book_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent { symbol: symbol.clone(), order_book, is_snapshot: true }));
                    }
                } else if channel.contains("trade") {
                    if let Ok(trades_data) = serde_json::from_value::<Vec<CoinspotWsTrade>>(data.clone()) {
                        let trades: Vec<Trade> = trades_data.iter().map(|t| Self::parse_trade(t, &symbol)).collect();
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades }));
                    } else if let Ok(trade_data) = serde_json::from_value::<CoinspotWsTrade>(data.clone()) {
                        let trade = Self::parse_trade(&trade_data, &symbol);
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent { symbol: symbol.clone(), trades: vec![trade] }));
                    }
                }
            }
        }

        Ok(())
    }

    async fn subscribe_stream(&mut self, channel: &str, coin: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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
            "coin": coin
        });
        ws_client.send(&sub_msg.to_string())?;

        let mut subs = self.subscriptions.write().await;
        subs.insert(format!("{channel}:{coin}"), coin.to_string());
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

impl Default for CoinspotWs { fn default() -> Self { Self::new() } }
impl Clone for CoinspotWs {
    fn clone(&self) -> Self {
        Self { ws_client: None, subscriptions: Arc::new(RwLock::new(HashMap::new())), event_tx: None }
    }
}

#[async_trait]
impl WsExchange for CoinspotWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ticker", &Self::format_symbol(symbol)).await
    }
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("orders", &Self::format_symbol(symbol)).await
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
struct CoinspotWsTicker {
    #[serde(default)] bid: Option<Decimal>,
    #[serde(default)] ask: Option<Decimal>,
    #[serde(default)] last: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotWsOrderEntry {
    #[serde(default)] rate: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotWsOrderBook {
    #[serde(default)] buyorders: Vec<CoinspotWsOrderEntry>,
    #[serde(default)] sellorders: Vec<CoinspotWsOrderEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotWsTrade {
    #[serde(default)] id: Option<String>,
    #[serde(default)] timestamp: Option<i64>,
    #[serde(default)] rate: Option<Decimal>,
    #[serde(default)] amount: Option<Decimal>,
    #[serde(default)] market: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_format_symbol() { assert_eq!(CoinspotWs::format_symbol("BTC/AUD"), "BTC"); }
    #[test] fn test_to_unified_symbol() { assert_eq!(CoinspotWs::to_unified_symbol("BTC"), "BTC/AUD"); }
    #[test] fn test_default() { let ws = CoinspotWs::default(); assert!(ws.ws_client.is_none()); }
    #[test] fn test_clone() { let ws = CoinspotWs::new(); assert!(ws.clone().ws_client.is_none()); }
    #[test] fn test_new() { let ws = CoinspotWs::new(); assert!(ws.ws_client.is_none()); }
    #[tokio::test] async fn test_ws_is_connected() { let ws = CoinspotWs::new(); assert!(!ws.ws_is_connected().await); }
    #[test]
    fn test_parse_ticker() {
        let data = CoinspotWsTicker {
            bid: Some(Decimal::from(65000)), ask: Some(Decimal::from(65500)),
            last: Some(Decimal::from(65250))
        };
        let ticker = CoinspotWs::parse_ticker(&data, "BTC/AUD");
        assert_eq!(ticker.symbol, "BTC/AUD");
    }
    #[test]
    fn test_parse_order_book() {
        let data = CoinspotWsOrderBook {
            buyorders: vec![CoinspotWsOrderEntry { rate: Some(Decimal::from(65000)), amount: Some(Decimal::from(2)) }],
            sellorders: vec![CoinspotWsOrderEntry { rate: Some(Decimal::from(65500)), amount: Some(Decimal::from(1)) }]
        };
        let ob = CoinspotWs::parse_order_book(&data, "BTC/AUD");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = CoinspotWsTrade {
            id: Some("123".into()), timestamp: Some(1704067200000),
            rate: Some(Decimal::from(65250)), amount: Some(Decimal::from(1)),
            market: Some("buy".into())
        };
        let trade = CoinspotWs::parse_trade(&data, "BTC/AUD");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"ticker","coin":"BTC","data":{"last":"65250"}}"#;
        assert!(CoinspotWs::process_message(msg, &tx).is_ok());
    }
}
