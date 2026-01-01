//! Coinone WebSocket Implementation
//!
//! Coinone 실시간 데이터 스트리밍 (Public only)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://stream.coinone.co.kr";

/// Coinone WebSocket 클라이언트
pub struct CoinoneWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl CoinoneWs {
    /// 새 Coinone WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// API 키와 시크릿으로 Coinone WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 통합 형식으로 변환 (BTC/KRW -> base=BTC, quote=KRW)
    fn parse_symbol(symbol: &str) -> (String, String) {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            (parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            // Fallback: 기본 KRW로 가정
            (symbol.to_uppercase(), "KRW".to_string())
        }
    }

    /// Coinone 심볼을 통합 심볼로 변환
    fn to_unified_symbol(target_currency: &str, quote_currency: &str) -> String {
        format!("{}/{}", target_currency.to_uppercase(), quote_currency.to_uppercase())
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinoneTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.target_currency, &data.quote_currency);
        let timestamp = data.timestamp;

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_best_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_best_qty.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_best_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_best_qty.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.first.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.target_volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &CoinoneOrderBookData) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.target_currency, &data.quote_currency);

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: b.price.parse().ok()?,
                amount: b.qty.parse().ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: a.price.parse().ok()?,
                amount: a.qty.parse().ok()?,
            })
        }).collect();

        let timestamp = data.timestamp;

        let order_book = OrderBook {
            symbol: symbol.clone(),
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
            symbol,
            order_book,
            is_snapshot: true, // Coinone은 항상 전체 스냅샷 전송
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &CoinoneTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.target_currency, &data.quote_currency);
        let timestamp = data.timestamp;

        let side = if data.is_seller_maker {
            Some("sell".to_string())
        } else {
            Some("buy".to_string())
        };

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.qty.parse().unwrap_or_default();

        let trade = Trade {
            id: data.id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // JSON 파싱
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // 에러 체크
        let response_type = json.get("response_type")?.as_str()?;
        if response_type == "ERROR" {
            let error_code = json.get("error_code").and_then(|v| v.as_i64()).unwrap_or(0);
            let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            return Some(WsMessage::Error(format!("Error {}: {}", error_code, message)));
        }

        // PONG 처리
        if response_type == "PONG" {
            return None; // PONG은 클라이언트가 자동 처리
        }

        // DATA 처리
        if response_type == "DATA" {
            let channel = json.get("channel")?.as_str()?;

            match channel {
                "TICKER" => {
                    if let Ok(response) = serde_json::from_str::<CoinoneTickerResponse>(msg) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&response.data)));
                    }
                }
                "ORDERBOOK" => {
                    if let Ok(response) = serde_json::from_str::<CoinoneOrderBookResponse>(msg) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&response.data)));
                    }
                }
                "TRADE" => {
                    if let Ok(response) = serde_json::from_str::<CoinoneTradeResponse>(msg) {
                        return Some(WsMessage::Trade(Self::parse_trade(&response.data)));
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// 구독 메시지 생성
    fn create_subscribe_message(channel: &str, base: &str, quote: &str) -> String {
        let msg = CoinoneSubscribeRequest {
            request_type: "SUBSCRIBE".to_string(),
            channel: channel.to_string(),
            topic: CoinoneTopic {
                quote_currency: quote.to_uppercase(),
                target_currency: base.to_uppercase(),
            },
        };
        serde_json::to_string(&msg).unwrap_or_default()
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbol: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (base, quote) = Self::parse_symbol(symbol);

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_BASE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20, // 20초마다 PING
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol);
            self.subscriptions.write().await.insert(key, symbol.to_string());
        }

        // 구독 메시지 전송
        let subscribe_msg = Self::create_subscribe_message(channel, &base, &quote);
        if let Some(ref client) = self.ws_client {
            client.send(&subscribe_msg)?;
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

    /// PING 메시지 생성
    pub fn ping() -> String {
        r#"{"request_type":"PING"}"#.to_string()
    }
}

impl Default for CoinoneWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CoinoneWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for CoinoneWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("TICKER", symbol).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("ORDERBOOK", symbol).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("TRADE", symbol).await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchOHLCV".into(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Coinone은 구독시 자동 연결
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

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // Coinone WebSocket은 인증 스트림을 지원하지 않음
        Err(CcxtError::NotSupported {
            feature: "ws_authenticate".into(),
        })
    }
}

// === Coinone WebSocket Message Types ===

/// 구독 요청
#[derive(Debug, Serialize)]
struct CoinoneSubscribeRequest {
    request_type: String,
    channel: String,
    topic: CoinoneTopic,
}

/// 구독 토픽
#[derive(Debug, Serialize, Deserialize)]
struct CoinoneTopic {
    quote_currency: String,
    target_currency: String,
}

/// 티커 응답
#[derive(Debug, Deserialize)]
struct CoinoneTickerResponse {
    response_type: String,
    channel: String,
    data: CoinoneTickerData,
}

/// 티커 데이터
#[derive(Debug, Deserialize, Serialize)]
struct CoinoneTickerData {
    quote_currency: String,
    target_currency: String,
    timestamp: i64,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    target_volume: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    first: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    volume_power: Option<String>,
    #[serde(default)]
    ask_best_price: Option<String>,
    #[serde(default)]
    ask_best_qty: Option<String>,
    #[serde(default)]
    bid_best_price: Option<String>,
    #[serde(default)]
    bid_best_qty: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    yesterday_high: Option<String>,
    #[serde(default)]
    yesterday_low: Option<String>,
    #[serde(default)]
    yesterday_first: Option<String>,
    #[serde(default)]
    yesterday_last: Option<String>,
    #[serde(default)]
    yesterday_quote_volume: Option<String>,
    #[serde(default)]
    yesterday_target_volume: Option<String>,
}

/// 호가창 응답
#[derive(Debug, Deserialize)]
struct CoinoneOrderBookResponse {
    response_type: String,
    channel: String,
    data: CoinoneOrderBookData,
}

/// 호가창 데이터
#[derive(Debug, Deserialize, Serialize)]
struct CoinoneOrderBookData {
    quote_currency: String,
    target_currency: String,
    timestamp: i64,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    asks: Vec<CoinoneOrderBookEntry>,
    #[serde(default)]
    bids: Vec<CoinoneOrderBookEntry>,
}

/// 호가창 엔트리
#[derive(Debug, Deserialize, Serialize)]
struct CoinoneOrderBookEntry {
    price: String,
    qty: String,
}

/// 체결 응답
#[derive(Debug, Deserialize)]
struct CoinoneTradeResponse {
    response_type: String,
    channel: String,
    data: CoinoneTradeData,
}

/// 체결 데이터
#[derive(Debug, Deserialize, Serialize)]
struct CoinoneTradeData {
    quote_currency: String,
    target_currency: String,
    id: String,
    timestamp: i64,
    price: String,
    qty: String,
    is_seller_maker: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_symbol() {
        let (base, quote) = CoinoneWs::parse_symbol("BTC/KRW");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "KRW");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinoneWs::to_unified_symbol("BTC", "KRW"), "BTC/KRW");
        assert_eq!(CoinoneWs::to_unified_symbol("ETH", "KRW"), "ETH/KRW");
    }

    #[test]
    fn test_create_subscribe_message() {
        let msg = CoinoneWs::create_subscribe_message("TICKER", "BTC", "KRW");
        assert!(msg.contains("SUBSCRIBE"));
        assert!(msg.contains("TICKER"));
        assert!(msg.contains("BTC"));
        assert!(msg.contains("KRW"));
    }

    #[test]
    fn test_ping_message() {
        let ping = CoinoneWs::ping();
        assert_eq!(ping, r#"{"request_type":"PING"}"#);
    }

    #[test]
    fn test_parse_ticker() {
        let json = r#"{
            "response_type": "DATA",
            "channel": "TICKER",
            "data": {
                "quote_currency": "KRW",
                "target_currency": "BTC",
                "timestamp": 1705301117198,
                "quote_volume": "19521465345.504",
                "target_volume": "334.81445168",
                "high": "58710000",
                "low": "57276000",
                "first": "57293000",
                "last": "58532000",
                "volume_power": "100",
                "ask_best_price": "58537000",
                "ask_best_qty": "0.1961",
                "bid_best_price": "58532000",
                "bid_best_qty": "0.00009258"
            }
        }"#;

        if let Ok(response) = serde_json::from_str::<CoinoneTickerResponse>(json) {
            let event = CoinoneWs::parse_ticker(&response.data);
            assert_eq!(event.symbol, "BTC/KRW");
            assert_eq!(event.ticker.last, Some(Decimal::new(58532000, 0)));
        }
    }

    #[test]
    fn test_parse_orderbook() {
        let json = r#"{
            "response_type": "DATA",
            "channel": "ORDERBOOK",
            "data": {
                "quote_currency": "KRW",
                "target_currency": "BTC",
                "timestamp": 1705288918649,
                "id": "1705288918649001",
                "asks": [
                    {"price": "58412000", "qty": "0.59919807"}
                ],
                "bids": [
                    {"price": "58292000", "qty": "0.1045"}
                ]
            }
        }"#;

        if let Ok(response) = serde_json::from_str::<CoinoneOrderBookResponse>(json) {
            let event = CoinoneWs::parse_order_book(&response.data);
            assert_eq!(event.symbol, "BTC/KRW");
            assert_eq!(event.order_book.asks.len(), 1);
            assert_eq!(event.order_book.bids.len(), 1);
        }
    }

    #[test]
    fn test_parse_trade() {
        let json = r#"{
            "response_type": "DATA",
            "channel": "TRADE",
            "data": {
                "quote_currency": "KRW",
                "target_currency": "BTC",
                "id": "1705303667916001",
                "timestamp": 1705303667916,
                "price": "58490000",
                "qty": "0.0008",
                "is_seller_maker": false
            }
        }"#;

        if let Ok(response) = serde_json::from_str::<CoinoneTradeResponse>(json) {
            let event = CoinoneWs::parse_trade(&response.data);
            assert_eq!(event.symbol, "BTC/KRW");
            assert_eq!(event.trades.len(), 1);
            assert_eq!(event.trades[0].side, Some("buy".to_string()));
        }
    }
}
