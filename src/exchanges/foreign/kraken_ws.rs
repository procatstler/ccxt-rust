//! Kraken WebSocket Implementation
//!
//! Kraken 실시간 데이터 스트리밍

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
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws.kraken.com";

/// Kraken WebSocket 클라이언트
pub struct KrakenWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl KrakenWs {
    /// 새 Kraken WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Kraken WebSocket 형식으로 변환 (BTC/USD -> XBT/USD)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("BTC", "XBT")
    }

    /// Kraken 심볼을 통합 심볼로 변환 (XBT/USD -> BTC/USD)
    fn to_unified_symbol(kraken_symbol: &str) -> String {
        kraken_symbol.replace("XBT", "BTC")
    }

    /// Timeframe을 Kraken 포맷으로 변환 (분 단위)
    fn format_interval(timeframe: Timeframe) -> i32 {
        match timeframe {
            Timeframe::Minute1 => 1,
            Timeframe::Minute5 => 5,
            Timeframe::Minute15 => 15,
            Timeframe::Minute30 => 30,
            Timeframe::Hour1 => 60,
            Timeframe::Hour4 => 240,
            Timeframe::Day1 => 1440,
            Timeframe::Week1 => 10080,
            _ => 1,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KrakenTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.h.as_ref().and_then(|h| h.get(1)).and_then(|v| Decimal::from_str(v).ok()),
            low: data.l.as_ref().and_then(|l| l.get(1)).and_then(|v| Decimal::from_str(v).ok()),
            bid: data.b.as_ref().and_then(|b| b.first()).and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data.b.as_ref().and_then(|b| b.get(2)).and_then(|v| Decimal::from_str(v).ok()),
            ask: data.a.as_ref().and_then(|a| a.first()).and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data.a.as_ref().and_then(|a| a.get(2)).and_then(|v| Decimal::from_str(v).ok()),
            vwap: data.p.as_ref().and_then(|p| p.get(1)).and_then(|v| Decimal::from_str(v).ok()),
            open: data.o.as_ref().and_then(|o| o.first()).and_then(|v| Decimal::from_str(v).ok()),
            close: data.c.as_ref().and_then(|c| c.first()).and_then(|v| Decimal::from_str(v).ok()),
            last: data.c.as_ref().and_then(|c| c.first()).and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.get(1)).and_then(|val| Decimal::from_str(val).ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &KrakenOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&e[0]).ok()?,
                        amount: Decimal::from_str(&e[1]).ok()?,
                    })
                } else {
                    None
                }
            }).collect()
        };

        let bids = data.bs.as_ref().map(parse_entries)
            .or_else(|| data.b.as_ref().map(parse_entries))
            .unwrap_or_default();

        let asks = data.as_.as_ref().map(parse_entries)
            .or_else(|| data.a.as_ref().map(parse_entries))
            .unwrap_or_default();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trades(data: &[Vec<String>], symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let trades: Vec<Trade> = data.iter().filter_map(|t| {
            if t.len() < 6 {
                return None;
            }

            let price = Decimal::from_str(&t[0]).ok()?;
            let amount = Decimal::from_str(&t[1]).ok()?;
            let timestamp = t[2].parse::<f64>().ok().map(|ts| (ts * 1000.0) as i64)?;
            let side = match t[3].as_str() {
                "b" => Some("buy".to_string()),
                "s" => Some("sell".to_string()),
                _ => None,
            };

            Some(Trade {
                id: t[2].clone(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side,
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::Value::Array(t.iter().map(|s| serde_json::Value::String(s.clone())).collect()),
            })
        }).collect();

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse the JSON message
        // Kraken sends array format: [channelID, data, "channelName", "pair"]

        // Try to parse as subscription status
        if let Ok(status) = serde_json::from_str::<KrakenWsStatus>(msg) {
            if status.status.as_deref() == Some("subscribed") {
                return Some(WsMessage::Subscribed {
                    channel: status.subscription.as_ref()
                        .and_then(|s| s.name.clone())
                        .unwrap_or_default(),
                    symbol: status.pair.clone(),
                });
            }
            return None;
        }

        // Try to parse as array data
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(msg) {
            if arr.len() >= 4 {
                let channel_name = arr.get(arr.len() - 2).and_then(|v| v.as_str());
                let pair = arr.last().and_then(|v| v.as_str()).unwrap_or("");

                match channel_name {
                    Some("ticker") => {
                        if let Some(data) = arr.get(1) {
                            if let Ok(ticker_data) = serde_json::from_value::<KrakenTickerData>(data.clone()) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, pair)));
                            }
                        }
                    }
                    Some(name) if name.starts_with("book") => {
                        if let Some(data) = arr.get(1) {
                            if let Ok(ob_data) = serde_json::from_value::<KrakenOrderBookData>(data.clone()) {
                                let is_snapshot = ob_data.bs.is_some() || ob_data.as_.is_some();
                                return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, pair, is_snapshot)));
                            }
                        }
                    }
                    Some("trade") => {
                        if let Some(data) = arr.get(1) {
                            if let Ok(trade_data) = serde_json::from_value::<Vec<Vec<String>>>(data.clone()) {
                                return Some(WsMessage::Trade(Self::parse_trades(&trade_data, pair)));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        pairs: Vec<String>,
        channel: &str,
        depth: Option<i32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let mut subscription = serde_json::json!({
            "name": channel
        });

        if let Some(d) = depth {
            subscription["depth"] = serde_json::json!(d);
        }

        let subscribe_msg = serde_json::json!({
            "event": "subscribe",
            "pair": pairs,
            "subscription": subscription
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, pairs.join(","));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    }
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for KrakenWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for KrakenWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![kraken_symbol], "ticker", None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(kraken_symbols, "ticker", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        let depth = match limit.unwrap_or(10) {
            1..=10 => 10,
            11..=25 => 25,
            26..=100 => 100,
            101..=500 => 500,
            _ => 1000,
        };
        client.subscribe_stream(vec![kraken_symbol], &format!("book-{depth}"), Some(depth)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![kraken_symbol], "trade", None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let kraken_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        client.subscribe_stream(vec![kraken_symbol], &format!("ohlc-{interval}"), None).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = &self.ws_client {
            client.is_connected().await
        } else {
            false
        }
    }
}

// === Kraken WebSocket Types ===

#[derive(Debug, Deserialize)]
struct KrakenWsStatus {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    subscription: Option<KrakenWsSubscription>,
}

#[derive(Debug, Deserialize)]
struct KrakenWsSubscription {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    depth: Option<i32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct KrakenTickerData {
    #[serde(default)]
    a: Option<Vec<String>>, // ask [price, wholeLotVolume, lotVolume]
    #[serde(default)]
    b: Option<Vec<String>>, // bid [price, wholeLotVolume, lotVolume]
    #[serde(default)]
    c: Option<Vec<String>>, // close [price, lotVolume]
    #[serde(default)]
    v: Option<Vec<String>>, // volume [today, last24Hours]
    #[serde(default)]
    p: Option<Vec<String>>, // vwap [today, last24Hours]
    #[serde(default)]
    t: Option<Vec<i64>>, // trade count [today, last24Hours]
    #[serde(default)]
    l: Option<Vec<String>>, // low [today, last24Hours]
    #[serde(default)]
    h: Option<Vec<String>>, // high [today, last24Hours]
    #[serde(default)]
    o: Option<Vec<String>>, // open [today, last24Hours]
}

#[derive(Debug, Default, Deserialize)]
struct KrakenOrderBookData {
    #[serde(default)]
    bs: Option<Vec<Vec<String>>>, // bids snapshot
    #[serde(default, rename = "as")]
    as_: Option<Vec<Vec<String>>>, // asks snapshot
    #[serde(default)]
    b: Option<Vec<Vec<String>>>, // bids update
    #[serde(default)]
    a: Option<Vec<Vec<String>>>, // asks update
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(KrakenWs::format_symbol("BTC/USD"), "XBT/USD");
        assert_eq!(KrakenWs::format_symbol("ETH/BTC"), "ETH/XBT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KrakenWs::to_unified_symbol("XBT/USD"), "BTC/USD");
        assert_eq!(KrakenWs::to_unified_symbol("ETH/XBT"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KrakenWs::format_interval(Timeframe::Minute1), 1);
        assert_eq!(KrakenWs::format_interval(Timeframe::Hour1), 60);
        assert_eq!(KrakenWs::format_interval(Timeframe::Day1), 1440);
    }
}
