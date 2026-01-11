//! BTCMarkets WebSocket Implementation
//!
//! Australian cryptocurrency exchange WebSocket streams

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

const WS_URL: &str = "wss://socket.btcmarkets.net/v2";

/// BTCMarkets WebSocket client
pub struct BtcmarketsWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BtcmarketsWs {
    /// Create new BTCMarkets WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Convert unified symbol to BTCMarkets format (BTC/AUD -> BTC-AUD)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert BTCMarkets symbol to unified format (BTC-AUD -> BTC/AUD)
    fn to_unified_symbol(market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// Parse ticker data
    fn parse_ticker(data: &BtcmarketsWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone().or_else(|| Some(Utc::now().to_rfc3339())),
            high: data.high24h,
            low: data.low24h,
            bid: data.best_bid,
            bid_volume: None,
            ask: data.best_ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_price,
            last: data.last_price,
            previous_close: None,
            change: None,
            percentage: data.price_pct_24h,
            average: None,
            base_volume: data.volume24h,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book data
    fn parse_order_book(data: &BtcmarketsWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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
            datetime: data.timestamp.clone().or_else(|| Some(Utc::now().to_rfc3339())),
            nonce: None,
            bids,
            asks,
        }
    }

    /// Parse trade data
    fn parse_trade(data: &BtcmarketsWsTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);

        Trade {
            id: data.id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone().or_else(|| Some(Utc::now().to_rfc3339())),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.as_ref().map(|s| s.to_lowercase()),
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

        // Check message type
        if let Some(msg_type) = json.get("messageType").and_then(|t| t.as_str()) {
            let market_id = json.get("marketId").and_then(|m| m.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(market_id);

            match msg_type {
                "tick" => {
                    if let Ok(ticker_data) = serde_json::from_value::<BtcmarketsWsTicker>(json.clone()) {
                        let ticker = Self::parse_ticker(&ticker_data, &symbol);
                        let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                            symbol: symbol.clone(),
                            ticker,
                        }));
                    }
                }
                "orderbook" => {
                    if let Ok(orderbook_data) = serde_json::from_value::<BtcmarketsWsOrderBook>(json.clone()) {
                        let order_book = Self::parse_order_book(&orderbook_data, &symbol);
                        let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                            symbol: symbol.clone(),
                            order_book,
                            is_snapshot: true,
                        }));
                    }
                }
                "trade" => {
                    if let Ok(trade_data) = serde_json::from_value::<BtcmarketsWsTrade>(json.clone()) {
                        let trade = Self::parse_trade(&trade_data, &symbol);
                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                            symbol: symbol.clone(),
                            trades: vec![trade],
                        }));
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
            "messageType": "subscribe",
            "marketIds": [market_id],
            "channels": [channel]
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

impl Default for BtcmarketsWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BtcmarketsWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for BtcmarketsWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("tick", &market_id).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("orderbook", &market_id).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let market_id = Self::format_symbol(symbol);
        ws.subscribe_stream("trade", &market_id).await
    }

    async fn watch_ohlcv(&self, symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // BTCMarkets doesn't support OHLCV WebSocket natively
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
#[serde(rename_all = "camelCase")]
struct BtcmarketsWsTicker {
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    last_price: Option<Decimal>,
    #[serde(default)]
    volume24h: Option<Decimal>,
    #[serde(default)]
    price_pct_24h: Option<Decimal>,
    #[serde(default)]
    low24h: Option<Decimal>,
    #[serde(default)]
    high24h: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcmarketsWsOrderBook {
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BtcmarketsWsTrade {
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    id: Option<String>,
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
        assert_eq!(BtcmarketsWs::format_symbol("BTC/AUD"), "BTC-AUD");
        assert_eq!(BtcmarketsWs::format_symbol("ETH/AUD"), "ETH-AUD");
        assert_eq!(BtcmarketsWs::format_symbol("XRP/BTC"), "XRP-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BtcmarketsWs::to_unified_symbol("BTC-AUD"), "BTC/AUD");
        assert_eq!(BtcmarketsWs::to_unified_symbol("ETH-AUD"), "ETH/AUD");
        assert_eq!(BtcmarketsWs::to_unified_symbol("XRP-BTC"), "XRP/BTC");
    }

    #[test]
    fn test_default() {
        let ws = BtcmarketsWs::default();
        assert!(ws.ws_client.is_none());
        assert!(ws.event_tx.is_none());
    }

    #[test]
    fn test_clone() {
        let ws = BtcmarketsWs::new();
        let cloned = ws.clone();
        assert!(cloned.ws_client.is_none());
    }

    #[test]
    fn test_parse_ticker() {
        let data = BtcmarketsWsTicker {
            market_id: Some("BTC-AUD".to_string()),
            timestamp: Some("2024-01-01T00:00:00Z".to_string()),
            best_bid: Some(Decimal::from_str("50000").unwrap()),
            best_ask: Some(Decimal::from_str("50100").unwrap()),
            last_price: Some(Decimal::from_str("50050").unwrap()),
            volume24h: Some(Decimal::from_str("100").unwrap()),
            price_pct_24h: Some(Decimal::from_str("2.5").unwrap()),
            low24h: Some(Decimal::from_str("49000").unwrap()),
            high24h: Some(Decimal::from_str("51000").unwrap()),
        };
        let ticker = BtcmarketsWs::parse_ticker(&data, "BTC/AUD");
        assert_eq!(ticker.symbol, "BTC/AUD");
        assert_eq!(ticker.last, Some(Decimal::from_str("50050").unwrap()));
    }

    #[test]
    fn test_parse_order_book() {
        let data = BtcmarketsWsOrderBook {
            market_id: Some("BTC-AUD".to_string()),
            timestamp: Some("2024-01-01T00:00:00Z".to_string()),
            bids: vec![
                vec!["50000".to_string(), "1.5".to_string()],
                vec!["49900".to_string(), "2.0".to_string()],
            ],
            asks: vec![
                vec!["50100".to_string(), "1.0".to_string()],
                vec!["50200".to_string(), "1.5".to_string()],
            ],
        };
        let order_book = BtcmarketsWs::parse_order_book(&data, "BTC/AUD");
        assert_eq!(order_book.symbol, "BTC/AUD");
        assert_eq!(order_book.bids.len(), 2);
        assert_eq!(order_book.asks.len(), 2);
    }

    #[test]
    fn test_parse_trade() {
        let data = BtcmarketsWsTrade {
            market_id: Some("BTC-AUD".to_string()),
            timestamp: Some("2024-01-01T00:00:00Z".to_string()),
            id: Some("123456".to_string()),
            price: Some(Decimal::from_str("50050").unwrap()),
            amount: Some(Decimal::from_str("0.5").unwrap()),
            side: Some("Bid".to_string()),
        };
        let trade = BtcmarketsWs::parse_trade(&data, "BTC/AUD");
        assert_eq!(trade.symbol, "BTC/AUD");
        assert_eq!(trade.id, "123456");
        assert_eq!(trade.price, Decimal::from_str("50050").unwrap());
    }

    #[test]
    fn test_new() {
        let ws = BtcmarketsWs::new();
        assert!(ws.ws_client.is_none());
    }

    #[tokio::test]
    async fn test_ws_is_connected_when_no_client() {
        let ws = BtcmarketsWs::new();
        assert!(!ws.ws_is_connected().await);
    }

    #[test]
    fn test_process_ticker_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"messageType":"tick","marketId":"BTC-AUD","bestBid":"50000","bestAsk":"50100","lastPrice":"50050","timestamp":"2024-01-01T00:00:00Z"}"#;
        let result = BtcmarketsWs::process_message(msg, &tx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_orderbook_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"messageType":"orderbook","marketId":"BTC-AUD","bids":[["50000","1.5"]],"asks":[["50100","1.0"]],"timestamp":"2024-01-01T00:00:00Z"}"#;
        let result = BtcmarketsWs::process_message(msg, &tx);
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_trade_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"messageType":"trade","marketId":"BTC-AUD","id":"123456","price":"50050","amount":"0.5","side":"Bid","timestamp":"2024-01-01T00:00:00Z"}"#;
        let result = BtcmarketsWs::process_message(msg, &tx);
        assert!(result.is_ok());
    }
}
