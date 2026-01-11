//! Indodax WebSocket Implementation
//!
//! Indonesian cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://indodax.com/ws/";

/// Indodax WebSocket client
pub struct IndodaxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl IndodaxWs {
    /// Create new Indodax WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Convert unified symbol to Indodax format (BTC/IDR -> btcidr)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// Convert Indodax symbol to unified format (btcidr -> BTC/IDR)
    fn to_unified_symbol(market_id: &str) -> String {
        // Common quote currencies
        let quotes = ["idr", "usdt", "btc", "eth"];
        let lower = market_id.to_lowercase();

        for quote in &quotes {
            if lower.ends_with(quote) {
                let base = &lower[..lower.len() - quote.len()];
                return format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
            }
        }

        // Fallback
        if lower.len() > 3 {
            let base = &lower[..lower.len() - 3];
            let quote = &lower[lower.len() - 3..];
            return format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
        }

        market_id.to_uppercase()
    }

    /// Parse ticker data
    fn parse_ticker(data: &IndodaxWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp.unwrap_or(0) * 1000;
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
            base_volume: data.vol_base,
            quote_volume: data.vol_quote,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book data
    fn parse_order_book(data: &IndodaxWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&entry[0]).ok()?,
                        amount: Decimal::from_str(&entry[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&entry[0]).ok()?,
                        amount: Decimal::from_str(&entry[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        }
    }

    /// Parse trade data
    fn parse_trade(data: &IndodaxWsTrade, symbol: &str) -> Trade {
        let timestamp = data.date.unwrap_or(0) * 1000;
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);

        Trade {
            id: data.tid.map(|t| t.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
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

    /// Process WebSocket message
    fn process_message(msg: &str, event_tx: &mpsc::UnboundedSender<WsMessage>) -> CcxtResult<()> {
        let json: serde_json::Value = serde_json::from_str(msg)?;

        // Check for ticker message
        if let Some(channel) = json.get("channel").and_then(|c| c.as_str()) {
            let market = json.get("market").and_then(|m| m.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(market);

            match channel {
                "ticker" => {
                    if let Some(data) = json.get("data") {
                        if let Ok(ticker_data) = serde_json::from_value::<IndodaxWsTicker>(data.clone()) {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                                symbol: symbol.clone(),
                                ticker,
                            }));
                        }
                    }
                }
                "depth" => {
                    if let Some(data) = json.get("data") {
                        if let Ok(depth_data) = serde_json::from_value::<IndodaxWsOrderBook>(data.clone()) {
                            let order_book = Self::parse_order_book(&depth_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                symbol: symbol.clone(),
                                order_book,
                                is_snapshot: true,
                            }));
                        }
                    }
                }
                "trades" => {
                    if let Some(data) = json.get("data") {
                        if let Ok(trades_data) = serde_json::from_value::<Vec<IndodaxWsTrade>>(data.clone()) {
                            let trades: Vec<Trade> = trades_data
                                .iter()
                                .map(|t| Self::parse_trade(t, &symbol))
                                .collect();
                            let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                symbol: symbol.clone(),
                                trades,
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Subscribe to a stream
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
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let sub_msg = serde_json::json!({
            "action": "subscribe",
            "channel": channel,
            "market": market_id
        });

        ws_client.send(&sub_msg.to_string())?;

        // Store subscription
        let mut subs = self.subscriptions.write().await;
        subs.insert(format!("{channel}:{market_id}"), market_id.to_string());
        drop(subs);

        // Spawn message handler
        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        let _ = Self::process_message(&msg, &event_tx);
                    }
                    WsEvent::Connected => {
                        let _ = event_tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = event_tx.send(WsMessage::Disconnected);
                        break;
                    }
                    WsEvent::Error(e) => {
                        let _ = event_tx.send(WsMessage::Error(e));
                    }
                    WsEvent::Ping | WsEvent::Pong => {}
                }
            }

            // Cleanup subscriptions
            let mut subs = subscriptions.write().await;
            subs.clear();
        });

        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }
}

impl Default for IndodaxWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for IndodaxWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for IndodaxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("ticker", &market_id).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("depth", &market_id).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("trades", &market_id).await
    }

    async fn watch_ohlcv(&self, symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Indodax doesn't support OHLCV WebSocket natively
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

// === WebSocket Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxWsTicker {
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    buy: Option<Decimal>,
    #[serde(default)]
    sell: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default, rename = "vol_base")]
    vol_base: Option<Decimal>,
    #[serde(default, rename = "vol_quote")]
    vol_quote: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxWsOrderBook {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IndodaxWsTrade {
    #[serde(default)]
    tid: Option<i64>,
    #[serde(default)]
    date: Option<i64>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default, rename = "type")]
    trade_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(IndodaxWs::format_symbol("BTC/IDR"), "btcidr");
        assert_eq!(IndodaxWs::format_symbol("ETH/USDT"), "ethusdt");
        assert_eq!(IndodaxWs::format_symbol("DOGE/BTC"), "dogebtc");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(IndodaxWs::to_unified_symbol("btcidr"), "BTC/IDR");
        assert_eq!(IndodaxWs::to_unified_symbol("ethusdt"), "ETH/USDT");
        assert_eq!(IndodaxWs::to_unified_symbol("dogebtc"), "DOGE/BTC");
    }

    #[test]
    fn test_default() {
        let ws = IndodaxWs::default();
        assert!(ws.ws_client.is_none());
        assert!(ws.event_tx.is_none());
    }

    #[test]
    fn test_clone() {
        let ws = IndodaxWs::new();
        let cloned = ws.clone();
        assert!(cloned.ws_client.is_none());
    }

    #[test]
    fn test_parse_ticker() {
        let data = IndodaxWsTicker {
            high: Some(Decimal::from_str("50000000").unwrap()),
            low: Some(Decimal::from_str("48000000").unwrap()),
            buy: Some(Decimal::from_str("49500000").unwrap()),
            sell: Some(Decimal::from_str("49600000").unwrap()),
            last: Some(Decimal::from_str("49550000").unwrap()),
            vol_base: Some(Decimal::from_str("100").unwrap()),
            vol_quote: Some(Decimal::from_str("4950000000").unwrap()),
            timestamp: Some(1704067200),
        };
        let ticker = IndodaxWs::parse_ticker(&data, "BTC/IDR");
        assert_eq!(ticker.symbol, "BTC/IDR");
        assert_eq!(ticker.last, Some(Decimal::from_str("49550000").unwrap()));
    }

    #[test]
    fn test_parse_order_book() {
        let data = IndodaxWsOrderBook {
            bids: vec![
                vec!["49500000".to_string(), "1.5".to_string()],
                vec!["49400000".to_string(), "2.0".to_string()],
            ],
            asks: vec![
                vec!["49600000".to_string(), "1.0".to_string()],
                vec!["49700000".to_string(), "1.5".to_string()],
            ],
        };
        let order_book = IndodaxWs::parse_order_book(&data, "BTC/IDR");
        assert_eq!(order_book.symbol, "BTC/IDR");
        assert_eq!(order_book.bids.len(), 2);
        assert_eq!(order_book.asks.len(), 2);
    }

    #[test]
    fn test_parse_trade() {
        let data = IndodaxWsTrade {
            tid: Some(123456),
            date: Some(1704067200),
            price: Some(Decimal::from_str("49550000").unwrap()),
            amount: Some(Decimal::from_str("0.5").unwrap()),
            trade_type: Some("buy".to_string()),
        };
        let trade = IndodaxWs::parse_trade(&data, "BTC/IDR");
        assert_eq!(trade.symbol, "BTC/IDR");
        assert_eq!(trade.id, "123456");
        assert_eq!(trade.price, Decimal::from_str("49550000").unwrap());
    }

    #[test]
    fn test_new() {
        let ws = IndodaxWs::new();
        assert!(ws.ws_client.is_none());
    }

    #[tokio::test]
    async fn test_ws_is_connected_when_no_client() {
        let ws = IndodaxWs::new();
        assert!(!ws.ws_is_connected().await);
    }

    #[test]
    fn test_process_ticker_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"ticker","market":"btcidr","data":{"high":"50000000","low":"48000000","buy":"49500000","sell":"49600000","last":"49550000","timestamp":1704067200}}"#;
        let result = IndodaxWs::process_message(msg, &tx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_depth_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"depth","market":"btcidr","data":{"bids":[["49500000","1.5"]],"asks":[["49600000","1.0"]]}}"#;
        let result = IndodaxWs::process_message(msg, &tx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_trades_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"trades","market":"btcidr","data":[{"tid":123456,"date":1704067200,"price":"49550000","amount":"0.5","type":"buy"}]}"#;
        let result = IndodaxWs::process_message(msg, &tx);
        assert!(result.is_ok());
    }
}
