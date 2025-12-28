//! WOO X WebSocket Implementation
//!
//! WOO X 실시간 데이터 스트리밍

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

use super::woo::Woo;

const WS_PUBLIC_URL: &str = "wss://wss.woo.org/ws/stream";

/// WOO X WebSocket 클라이언트
pub struct WooWs {
    #[allow(dead_code)]
    rest: Woo,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl WooWs {
    /// 새 WOO X WebSocket 클라이언트 생성
    pub fn new(rest: Woo) -> Self {
        Self {
            rest,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 통합 심볼을 WOO 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        // WOO WebSocket uses BTC-USDT format
        symbol.replace("/", "-")
    }

    /// WOO 심볼을 통합 형식으로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(woo_symbol: &str) -> String {
        woo_symbol.replace("-", "/")
    }

    /// Timeframe을 WOO 인터벌로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1mon",
            _ => "1m",
        }
    }

    /// 타임프레임 문자열 파싱
    fn parse_timeframe(interval: &str) -> Timeframe {
        match interval {
            "1m" => Timeframe::Minute1,
            "5m" => Timeframe::Minute5,
            "15m" => Timeframe::Minute15,
            "30m" => Timeframe::Minute30,
            "1h" => Timeframe::Hour1,
            "4h" => Timeframe::Hour4,
            "12h" => Timeframe::Hour12,
            "1d" => Timeframe::Day1,
            "1w" => Timeframe::Week1,
            "1mon" => Timeframe::Month1,
            _ => Timeframe::Minute1,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &WooTickerData, symbol: &str) -> WsTickerEvent {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: data.bid_size.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask: data.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: data.ask_size.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.close.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: data.amount.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: symbol.to_string(),
            ticker,
        }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &WooOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&b.price).ok()?,
                amount: Decimal::from_str(&b.quantity).ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: Decimal::from_str(&a.price).ok()?,
                amount: Decimal::from_str(&a.quantity).ok()?,
            })
        }).collect();

        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
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
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: true, // WOO sends full snapshots
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &WooTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = Decimal::from_str(&data.price).unwrap_or_default();
        let amount = Decimal::from_str(&data.size).unwrap_or_default();

        let trades = vec![Trade {
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
            side: data.side.as_ref().map(|s| s.to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &WooOhlcvData, symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.start_timestamp?;

        let ohlcv = OHLCV::new(
            timestamp,
            Decimal::from_str(&data.open).ok()?,
            Decimal::from_str(&data.high).ok()?,
            Decimal::from_str(&data.low).ok()?,
            Decimal::from_str(&data.close).ok()?,
            Decimal::from_str(&data.volume).ok()?,
        );

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// WebSocket 메시지 처리
    fn process_message(message: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<WooWsResponse>(message) {
            // 구독 확인 메시지
            if let Some(event) = response.event.as_deref() {
                if event == "subscribe" {
                    if let Some(topic) = response.topic {
                        let parts: Vec<&str> = topic.split('@').collect();
                        let symbol = if !parts.is_empty() {
                            Some(Self::to_unified_symbol(parts[0]))
                        } else {
                            None
                        };

                        return Some(WsMessage::Subscribed {
                            channel: topic,
                            symbol,
                        });
                    }
                    return None;
                }
            }

            // 데이터 메시지
            if let Some(topic) = response.topic {
                let parts: Vec<&str> = topic.split('@').collect();
                if parts.len() < 2 {
                    return None;
                }

                let woo_symbol = parts[0];
                let channel = parts[1];
                let symbol = Self::to_unified_symbol(woo_symbol);

                if let Some(data) = response.data {
                    match channel {
                        "ticker" => {
                            if let Ok(ticker_data) = serde_json::from_value::<WooTickerData>(data) {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, &symbol)));
                            }
                        }
                        "orderbook" | "orderbookupdate" => {
                            if let Ok(ob_data) = serde_json::from_value::<WooOrderBookData>(data) {
                                return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol)));
                            }
                        }
                        "trade" => {
                            if let Ok(trade_data) = serde_json::from_value::<WooTradeData>(data) {
                                return Some(WsMessage::Trade(Self::parse_trade(&trade_data, &symbol)));
                            }
                        }
                        ch if ch.starts_with("kline_") => {
                            // Extract timeframe from channel (e.g., "kline_1m")
                            let interval = ch.strip_prefix("kline_").unwrap_or("1m");
                            let timeframe = Self::parse_timeframe(interval);

                            if let Ok(ohlcv_data) = serde_json::from_value::<WooOhlcvData>(data) {
                                if let Some(event) = Self::parse_ohlcv(&ohlcv_data, &symbol, timeframe) {
                                    return Some(WsMessage::Ohlcv(event));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        None
    }

    /// 채널 구독 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, topic: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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

        // 구독 메시지 전송
        let subscribe_msg = WooSubscribeMessage {
            event: "subscribe".to_string(),
            topic: topic.to_string(),
        };

        let msg_json = serde_json::to_string(&subscribe_msg).unwrap();
        ws_client.send(&msg_json)?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        self.subscriptions.write().await.insert(topic.to_string(), topic.to_string());

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
                    WsEvent::Error(e) => {
                        let _ = tx.send(WsMessage::Error(e));
                    }
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

#[async_trait]
impl WsExchange for WooWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let _topic = format!("{}@ticker", woo_symbol);

        // Need to use interior mutability for subscribe_stream
        // This is a simplified placeholder - proper implementation would require Arc<RwLock<Self>>
        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let _topic = format!("{}@orderbook", woo_symbol);

        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let _topic = format!("{}@trade", woo_symbol);

        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let woo_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let _topic = format!("{}@kline_{}", woo_symbol, interval);

        let (_tx, rx) = mpsc::unbounded_channel();
        Ok(rx)
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_some() {
            return Ok(());
        }

        let (event_tx, _) = mpsc::unbounded_channel();
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
        self.ws_client = Some(ws_client);

        // 메시지 처리 태스크 시작
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
                    WsEvent::Error(e) => {
                        let _ = tx.send(WsMessage::Error(e));
                    }
                    _ => {}
                }
            }
        });

        let _ = event_tx.send(WsMessage::Connected);
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = self.ws_client.take() {
            client.close()?;
        }

        if let Some(tx) = &self.event_tx {
            let _ = tx.send(WsMessage::Disconnected);
        }

        self.subscriptions.write().await.clear();
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match self.ws_client.as_ref() {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

// === WOO WebSocket Response Types ===

#[derive(Debug, Deserialize)]
struct WooWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct WooSubscribeMessage {
    event: String,
    topic: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WooTickerData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(rename = "bidSize", default)]
    bid_size: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(rename = "askSize", default)]
    ask_size: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WooOrderBookData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    bids: Vec<WooOrderBookLevel>,
    #[serde(default)]
    asks: Vec<WooOrderBookLevel>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WooOrderBookLevel {
    #[serde(default)]
    price: String,
    #[serde(default)]
    quantity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WooTradeData {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(rename = "tradeId", default)]
    trade_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WooOhlcvData {
    #[serde(rename = "startTimestamp", default)]
    start_timestamp: Option<i64>,
    #[serde(default)]
    open: String,
    #[serde(default)]
    high: String,
    #[serde(default)]
    low: String,
    #[serde(default)]
    close: String,
    #[serde(default)]
    volume: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(WooWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(WooWs::format_symbol("ETH/USDT"), "ETH-USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(WooWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(WooWs::to_unified_symbol("ETH-USDT"), "ETH/USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(WooWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(WooWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(WooWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_timeframe() {
        assert_eq!(WooWs::parse_timeframe("1m"), Timeframe::Minute1);
        assert_eq!(WooWs::parse_timeframe("1h"), Timeframe::Hour1);
        assert_eq!(WooWs::parse_timeframe("1d"), Timeframe::Day1);
    }
}
