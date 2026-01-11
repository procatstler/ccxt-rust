//! Deepcoin WebSocket Implementation
//!
//! Cryptocurrency derivatives exchange WebSocket streams

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

const WS_URL: &str = "wss://stream.deepcoin.com/ws/public";

/// Deepcoin WebSocket client
pub struct DeepcoinWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl DeepcoinWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Deepcoin uses format like "BTC-USDT"
        symbol.replace("/", "-").to_uppercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    fn parse_ticker(data: &DeepcoinWsTicker, symbol: &str) -> Ticker {
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
            high: data.high_24h,
            low: data.low_24h,
            bid: data.best_bid,
            bid_volume: data.best_bid_size,
            ask: data.best_ask,
            ask_volume: data.best_ask_size,
            vwap: None,
            open: data.open_24h,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: data.change_24h,
            average: None,
            base_volume: data.volume_24h,
            quote_volume: data.turnover_24h,
            index_price: data.index_price,
            mark_price: data.mark_price,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &DeepcoinWsOrderBook, symbol: &str) -> OrderBook {
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

    fn parse_trade(data: &DeepcoinWsTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.size.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
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

        if let Some(arg) = json.get("arg") {
            let channel = arg.get("channel").and_then(|c| c.as_str()).unwrap_or("");
            let inst_id = arg.get("instId").and_then(|i| i.as_str()).unwrap_or("");
            let symbol = Self::to_unified_symbol(inst_id);

            if let Some(data) = json.get("data") {
                if channel.contains("ticker") {
                    if let Some(first) = data.as_array().and_then(|arr| arr.first()) {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<DeepcoinWsTicker>(first.clone())
                        {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                                symbol: symbol.clone(),
                                ticker,
                            }));
                        }
                    }
                } else if channel.contains("books") {
                    if let Some(first) = data.as_array().and_then(|arr| arr.first()) {
                        if let Ok(book_data) =
                            serde_json::from_value::<DeepcoinWsOrderBook>(first.clone())
                        {
                            let order_book = Self::parse_order_book(&book_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                symbol: symbol.clone(),
                                order_book,
                                is_snapshot: true,
                            }));
                        }
                    }
                } else if channel.contains("trades") {
                    if let Ok(trades_data) =
                        serde_json::from_value::<Vec<DeepcoinWsTrade>>(data.clone())
                    {
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

        let sub_msg = serde_json::json!({
            "op": "subscribe",
            "args": [{
                "channel": channel,
                "instId": market_id
            }]
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

impl Default for DeepcoinWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for DeepcoinWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for DeepcoinWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("tickers", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("books", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("trades", &Self::format_symbol(symbol))
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
struct DeepcoinWsTicker {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "high24h")]
    high_24h: Option<Decimal>,
    #[serde(default, rename = "low24h")]
    low_24h: Option<Decimal>,
    #[serde(default, rename = "bestBid")]
    best_bid: Option<Decimal>,
    #[serde(default, rename = "bestBidSz")]
    best_bid_size: Option<Decimal>,
    #[serde(default, rename = "bestAsk")]
    best_ask: Option<Decimal>,
    #[serde(default, rename = "bestAskSz")]
    best_ask_size: Option<Decimal>,
    #[serde(default, rename = "open24h")]
    open_24h: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default, rename = "change24h")]
    change_24h: Option<Decimal>,
    #[serde(default, rename = "vol24h")]
    volume_24h: Option<Decimal>,
    #[serde(default, rename = "turnover24h")]
    turnover_24h: Option<Decimal>,
    #[serde(default, rename = "idxPx")]
    index_price: Option<Decimal>,
    #[serde(default, rename = "markPx")]
    mark_price: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepcoinWsOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeepcoinWsTrade {
    #[serde(default, rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "px")]
    price: Option<Decimal>,
    #[serde(default, rename = "sz")]
    size: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_symbol() {
        assert_eq!(DeepcoinWs::format_symbol("BTC/USDT"), "BTC-USDT");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DeepcoinWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
    }
    #[test]
    fn test_default() {
        let ws = DeepcoinWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = DeepcoinWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = DeepcoinWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = DeepcoinWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = DeepcoinWsTicker {
            timestamp: Some(1704067200000),
            high_24h: Some(Decimal::from(45000)),
            low_24h: Some(Decimal::from(43000)),
            best_bid: Some(Decimal::from(44500)),
            best_bid_size: Some(Decimal::from(2)),
            best_ask: Some(Decimal::from(44600)),
            best_ask_size: Some(Decimal::from(1)),
            open_24h: Some(Decimal::from(44000)),
            last: Some(Decimal::from(44550)),
            change_24h: None,
            volume_24h: Some(Decimal::from(100)),
            turnover_24h: None,
            index_price: None,
            mark_price: None,
        };
        let ticker = DeepcoinWs::parse_ticker(&data, "BTC/USDT");
        assert_eq!(ticker.symbol, "BTC/USDT");
    }
    #[test]
    fn test_parse_order_book() {
        let data = DeepcoinWsOrderBook {
            timestamp: Some(1704067200000),
            bids: vec![vec!["44500".into(), "1.5".into()]],
            asks: vec![vec!["44600".into(), "1.0".into()]],
        };
        let ob = DeepcoinWs::parse_order_book(&data, "BTC/USDT");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = DeepcoinWsTrade {
            trade_id: Some("123".into()),
            timestamp: Some(1704067200000),
            price: Some(Decimal::from(44550)),
            size: Some(Decimal::from(1)),
            side: Some("buy".into()),
        };
        let trade = DeepcoinWs::parse_trade(&data, "BTC/USDT");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"arg":{"channel":"tickers","instId":"BTC-USDT"},"data":[{"last":"44550"}]}"#;
        assert!(DeepcoinWs::process_message(msg, &tx).is_ok());
    }
}
