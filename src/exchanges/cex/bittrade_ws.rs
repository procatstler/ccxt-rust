//! Bittrade WebSocket Implementation
//!
//! Japanese cryptocurrency exchange WebSocket streams (Huobi Japan)

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

const WS_URL: &str = "wss://api-cloud.bittrade.co.jp/ws";

/// Bittrade WebSocket client
pub struct BittradeWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BittradeWs {
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    fn format_symbol(symbol: &str) -> String {
        // Bittrade uses format like "btcjpy"
        symbol.replace("/", "").to_lowercase()
    }

    fn to_unified_symbol(market_id: &str) -> String {
        // Convert "btcjpy" to "BTC/JPY"
        let upper = market_id.to_uppercase();
        if upper.len() >= 6 {
            let base = &upper[..3];
            let quote = &upper[3..];
            format!("{base}/{quote}")
        } else {
            upper
        }
    }

    fn parse_ticker(data: &BittradeWsTicker, symbol: &str) -> Ticker {
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());
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
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: None,
            open: data.open,
            close: data.close,
            last: data.close,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol,
            quote_volume: data.amount,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn parse_order_book(data: &BittradeWsOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());
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

    fn parse_trade(data: &BittradeWsTrade, symbol: &str) -> Trade {
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.unwrap_or(Decimal::ZERO);
        let amount = data.amount.unwrap_or(Decimal::ZERO);
        Trade {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.direction.clone(),
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

        if let Some(ch) = json.get("ch").and_then(|c| c.as_str()) {
            // Channel format: "market.btcjpy.ticker"
            let parts: Vec<&str> = ch.split('.').collect();
            if parts.len() >= 2 {
                let market = parts[1];
                let symbol = Self::to_unified_symbol(market);

                if let Some(tick) = json.get("tick") {
                    if ch.contains("ticker") || ch.contains("detail") {
                        if let Ok(ticker_data) =
                            serde_json::from_value::<BittradeWsTicker>(tick.clone())
                        {
                            let ticker = Self::parse_ticker(&ticker_data, &symbol);
                            let _ = event_tx.send(WsMessage::Ticker(WsTickerEvent {
                                symbol: symbol.clone(),
                                ticker,
                            }));
                        }
                    } else if ch.contains("depth") {
                        if let Ok(book_data) =
                            serde_json::from_value::<BittradeWsOrderBook>(tick.clone())
                        {
                            let order_book = Self::parse_order_book(&book_data, &symbol);
                            let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                symbol: symbol.clone(),
                                order_book,
                                is_snapshot: true,
                            }));
                        }
                    } else if ch.contains("trade") {
                        if let Some(data) = tick.get("data") {
                            if let Ok(trades_data) =
                                serde_json::from_value::<Vec<BittradeWsTrade>>(data.clone())
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
            "sub": format!("market.{}.{}", market_id, channel),
            "id": format!("sub_{}", market_id)
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

impl Default for BittradeWs {
    fn default() -> Self {
        Self::new()
    }
}
impl Clone for BittradeWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for BittradeWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("detail", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("depth.step0", &Self::format_symbol(symbol))
            .await
    }
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        ws.subscribe_stream("trade.detail", &Self::format_symbol(symbol))
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
struct BittradeWsTicker {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeWsOrderBook {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BittradeWsTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    direction: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_format_symbol() {
        assert_eq!(BittradeWs::format_symbol("BTC/JPY"), "btcjpy");
    }
    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BittradeWs::to_unified_symbol("btcjpy"), "BTC/JPY");
    }
    #[test]
    fn test_default() {
        let ws = BittradeWs::default();
        assert!(ws.ws_client.is_none());
    }
    #[test]
    fn test_clone() {
        let ws = BittradeWs::new();
        assert!(ws.clone().ws_client.is_none());
    }
    #[test]
    fn test_new() {
        let ws = BittradeWs::new();
        assert!(ws.ws_client.is_none());
    }
    #[tokio::test]
    async fn test_ws_is_connected() {
        let ws = BittradeWs::new();
        assert!(!ws.ws_is_connected().await);
    }
    #[test]
    fn test_parse_ticker() {
        let data = BittradeWsTicker {
            ts: Some(1704067200000),
            high: Some(Decimal::from(5000000)),
            low: Some(Decimal::from(4800000)),
            bid: Some(Decimal::from(4950000)),
            ask: Some(Decimal::from(4960000)),
            open: Some(Decimal::from(4900000)),
            close: Some(Decimal::from(4955000)),
            vol: Some(Decimal::from(100)),
            amount: Some(Decimal::from(495000000)),
        };
        let ticker = BittradeWs::parse_ticker(&data, "BTC/JPY");
        assert_eq!(ticker.symbol, "BTC/JPY");
    }
    #[test]
    fn test_parse_order_book() {
        let data = BittradeWsOrderBook {
            ts: Some(1704067200000),
            bids: vec![vec![Decimal::from(4950000), Decimal::from(2)]],
            asks: vec![vec![Decimal::from(4960000), Decimal::from(1)]],
        };
        let ob = BittradeWs::parse_order_book(&data, "BTC/JPY");
        assert_eq!(ob.bids.len(), 1);
    }
    #[test]
    fn test_parse_trade() {
        let data = BittradeWsTrade {
            id: Some(123),
            ts: Some(1704067200000),
            price: Some(Decimal::from(4955000)),
            amount: Some(Decimal::from(1)),
            direction: Some("buy".into()),
        };
        let trade = BittradeWs::parse_trade(&data, "BTC/JPY");
        assert_eq!(trade.id, "123");
    }
    #[test]
    fn test_process_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = r#"{"ch":"market.btcjpy.detail","tick":{"close":"4955000"}}"#;
        assert!(BittradeWs::process_message(msg, &tx).is_ok());
    }
}
