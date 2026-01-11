//! YoBit WebSocket Implementation
//!
//! Russian cryptocurrency exchange WebSocket streams

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_URL: &str = "wss://yobit.net/ws/";

/// YoBit WebSocket client
pub struct YobitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl YobitWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // YoBit uses format like "btc_usd"
        symbol.replace("/", "_").to_lowercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        market_id.to_uppercase().replace("_", "/")
    }

    fn parse_ticker(data: &YobitWsTicker, symbol: &str) -> Ticker {
        let timestamp = data
            .updated
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
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
            average: data.avg,
            base_volume: data.vol,
            quote_volume: data.vol_cur,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &YobitWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: e[0],
                        amount: e[1],
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
                        price: e[0],
                        amount: e[1],
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
            checksum: None,
        }
    }

    fn parse_trade(data: &YobitWsTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.tid.map(|i| i.to_string()).unwrap_or_default(),
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

    fn process_message(msg: &str, event_tx: &mpsc::UnboundedSender<WsMessage>) -> CcxtResult<()> {
        let json: serde_json::Value = serde_json::from_str(msg)?;

        if let Some(channel) = json.get("channel").and_then(|c| c.as_str()) {
            // Channel format: "ticker.btc_usd" or "trades.btc_usd"
            let parts: Vec<&str> = channel.split('.').collect();
            if parts.len() >= 2 {
                let channel_type = parts[0];
                let market = parts[1];
                let symbol = Self::to_unified_symbol(market);

                if let Some(data) = json.get("data") {
                    match channel_type {
                        "ticker" => {
                            if let Ok(ticker_data) =
                                serde_json::from_value::<YobitWsTicker>(data.clone())
                            {
                                let ticker = Self::parse_ticker(&ticker_data, &symbol);
                                let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                                    symbol: symbol.clone(),
                                    ticker,
                                }));
                            }
                        },
                        "depth" | "orderbook" => {
                            if let Ok(book_data) =
                                serde_json::from_value::<YobitWsOrderBook>(data.clone())
                            {
                                let order_book = Self::parse_order_book(&book_data, &symbol);
                                let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                    symbol: symbol.clone(),
                                    order_book,
                                    is_snapshot: true,
                                }));
                            }
                        },
                        "trades" => {
                            if let Ok(trades_data) =
                                serde_json::from_value::<Vec<YobitWsTrade>>(data.clone())
                            {
                                let trades: Vec<Trade> = trades_data
                                    .iter()
                                    .map(|t| Self::parse_trade(t, &symbol))
                                    .collect();
                                let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                    symbol: symbol.clone(),
                                    trades,
                                }));
                            } else if let Ok(trade_data) =
                                serde_json::from_value::<YobitWsTrade>(data.clone())
                            {
                                let trade = Self::parse_trade(&trade_data, &symbol);
                                let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                    symbol: symbol.clone(),
                                    trades: vec![trade],
                                }));
                            }
                        },
                        _ => {},
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
            "method": "subscribe",
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

impl Default for YobitWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for YobitWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for YobitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("ticker", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("depth", &Self::format_symbol(symbol))
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
struct YobitWsTicker {
    #[serde(default)]
    updated: Option<i64>,
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
    #[serde(default)]
    avg: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    vol_cur: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitWsOrderBook {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitWsTrade {
    #[serde(default)]
    tid: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
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
        assert_eq!(YobitWs::format_symbol("BTC/USD"), "btc_usd");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(YobitWs::to_unified_symbol("btc_usd"), "BTC/USD");
    }
    #[test]
    fn test_default() {
        let ws = YobitWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = YobitWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = YobitWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = YobitWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = YobitWsTicker {
            updated: Some(1704067200),
            high: Some(Decimal::from(45000)),
            low: Some(Decimal::from(43000)),
            buy: Some(Decimal::from(44500)),
            sell: Some(Decimal::from(44600)),
            last: Some(Decimal::from(44550)),
            avg: Some(Decimal::from(44000)),
            vol: Some(Decimal::from(100)),
            vol_cur: Some(Decimal::from(4450000)),
        };
        let ticker = YobitWs::parse_ticker(&data, "BTC/USD");
        assert_eq!(ticker.symbol, "BTC/USD");
    }
    #[test]
    fn test_parse_order_book() {
        let data = YobitWsOrderBook {
            bids: vec![vec![Decimal::from(44500), Decimal::from(2)]],
            asks: vec![vec![Decimal::from(44600), Decimal::from(1)]],
        };
        let ob = YobitWs::parse_order_book(&data, "BTC/USD");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = YobitWsTrade {
            tid: Some(123),
            timestamp: Some(1704067200),
            price: Some(Decimal::from(44550)),
            amount: Some(Decimal::from(1)),
            trade_type: Some("buy".into()),
        };
        let trade = YobitWs::parse_trade(&data, "BTC/USD");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"channel":"ticker.btc_usd","data":{"last":"44550"}}"#;
        assert!(YobitWs::process_message(msg, &tx).is_ok());
    }
}
