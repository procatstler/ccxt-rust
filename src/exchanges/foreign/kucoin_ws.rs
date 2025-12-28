//! Kucoin WebSocket Implementation
//!
//! Kucoin 실시간 데이터 스트리밍
//! Note: Kucoin requires a token from REST API before connecting to WebSocket

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

/// Kucoin WebSocket 클라이언트
pub struct KucoinWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    connect_id: String,
}

impl KucoinWs {
    /// 새 Kucoin WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            connect_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// 심볼을 Kucoin 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Kucoin 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(kucoin_symbol: &str) -> String {
        kucoin_symbol.replace("-", "/")
    }

    /// Timeframe을 Kucoin 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute3 => "3min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "1hour",
            Timeframe::Hour2 => "2hour",
            Timeframe::Hour4 => "4hour",
            Timeframe::Hour6 => "6hour",
            Timeframe::Hour8 => "8hour",
            Timeframe::Hour12 => "12hour",
            Timeframe::Day1 => "1day",
            Timeframe::Week1 => "1week",
            _ => "1min",
        }
    }

    /// Public 토큰 가져오기 (WebSocket 연결에 필요)
    async fn get_public_token() -> CcxtResult<KucoinWsToken> {
        let url = "https://api.kucoin.com/api/v1/bullet-public";
        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError { url: url.to_string(), message: e.to_string() })?;

        let token_response: KucoinTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError { data_type: "KucoinTokenResponse".to_string(), message: e.to_string() })?;

        token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get WebSocket token".into(),
        })
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KucoinTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.best_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change_price.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_rate.as_ref().and_then(|v| {
                v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))
            }),
            average: None,
            base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_value.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &KucoinOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
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
            nonce: data.sequence,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &KucoinTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        // Kucoin time is in nanoseconds
        let timestamp = data.time.map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
            taker_or_maker: data.taker_order_id.as_ref().map(|_| crate::types::TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &KucoinCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let candles = &data.candles;
        if candles.len() < 7 {
            return None;
        }

        let timestamp = candles[0].parse::<i64>().ok()? * 1000;
        let ohlcv = OHLCV::new(
            timestamp,
            candles[1].parse().ok()?,
            candles[3].parse().ok()?,
            candles[4].parse().ok()?,
            candles[2].parse().ok()?,
            candles[5].parse().ok()?,
        );

        let symbol = Self::to_unified_symbol(&data.symbol);

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Ping/pong handling
        if msg.contains("\"type\":\"pong\"") || msg.contains("\"type\":\"welcome\"") {
            return None;
        }

        if let Ok(response) = serde_json::from_str::<KucoinWsResponse>(msg) {
            // Ack message (subscription confirmation)
            if response.msg_type.as_deref() == Some("ack") {
                return Some(WsMessage::Subscribed {
                    channel: "subscription".to_string(),
                    symbol: None,
                });
            }

            // Message data
            if response.msg_type.as_deref() == Some("message") {
                if let (Some(topic), Some(data)) = (&response.topic, &response.data) {
                    // Ticker
                    if topic.starts_with("/market/ticker:") {
                        if let Ok(ticker_data) = serde_json::from_value::<KucoinTickerData>(data.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // Level2 OrderBook
                    if topic.starts_with("/market/level2:") {
                        if let Ok(ob_data) = serde_json::from_value::<KucoinOrderBookData>(data.clone()) {
                            let symbol_part = topic.trim_start_matches("/market/level2:");
                            let symbol = Self::to_unified_symbol(symbol_part);
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, false)));
                        }
                    }

                    // Match (trades)
                    if topic.starts_with("/market/match:") {
                        if let Ok(trade_data) = serde_json::from_value::<KucoinTradeData>(data.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }

                    // Candles
                    if topic.starts_with("/market/candles:") {
                        if let Ok(candle_data) = serde_json::from_value::<KucoinCandleData>(data.clone()) {
                            // Extract timeframe from topic (e.g., "/market/candles:BTC-USDT_1hour")
                            let parts: Vec<&str> = topic.split('_').collect();
                            let timeframe = if parts.len() >= 2 {
                                match parts[1] {
                                    "1min" => Timeframe::Minute1,
                                    "3min" => Timeframe::Minute3,
                                    "5min" => Timeframe::Minute5,
                                    "15min" => Timeframe::Minute15,
                                    "30min" => Timeframe::Minute30,
                                    "1hour" => Timeframe::Hour1,
                                    "2hour" => Timeframe::Hour2,
                                    "4hour" => Timeframe::Hour4,
                                    "6hour" => Timeframe::Hour6,
                                    "8hour" => Timeframe::Hour8,
                                    "12hour" => Timeframe::Hour12,
                                    "1day" => Timeframe::Day1,
                                    "1week" => Timeframe::Week1,
                                    _ => Timeframe::Minute1,
                                }
                            } else {
                                Timeframe::Minute1
                            };

                            if let Some(event) = Self::parse_candle(&candle_data, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, topics: Vec<String>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get WebSocket token
        let token_data = Self::get_public_token().await?;
        let server = token_data.instance_servers.first()
            .ok_or_else(|| CcxtError::ExchangeError { message: "No WebSocket server available".into() })?;

        let ws_url = format!("{}?token={}&connectId={}", server.endpoint, token_data.token, self.connect_id);

        let ping_interval = (server.ping_interval.unwrap_or(18000) / 1000) as u64;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: ping_interval,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let id = uuid::Uuid::new_v4().to_string();
        let subscribe_msg = serde_json::json!({
            "id": id,
            "type": "subscribe",
            "topic": topics.join(","),
            "privateChannel": false,
            "response": true
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
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

impl Default for KucoinWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for KucoinWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let topics = vec![format!("/market/ticker:{}", market_id)];
        client.subscribe_stream(topics, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        let topics = vec![format!("/market/ticker:{}", market_ids.join(","))];
        client.subscribe_stream(topics, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        // Use level2Depth5 for smaller updates, level2:50 for more depth
        let topics = vec![format!("/market/level2:{}", market_id)];
        client.subscribe_stream(topics, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let topics = vec![format!("/market/match:{}", market_id)];
        client.subscribe_stream(topics, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topics = vec![format!("/market/candles:{}_{}", market_id, interval)];
        client.subscribe_stream(topics, "ohlcv", Some(symbol)).await
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

// === Kucoin WebSocket Types ===

#[derive(Debug, Deserialize)]
struct KucoinTokenResponse {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    data: Option<KucoinWsToken>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinWsToken {
    token: String,
    instance_servers: Vec<KucoinWsServer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinWsServer {
    endpoint: String,
    #[serde(default)]
    ping_interval: Option<i64>,
    #[serde(default)]
    ping_timeout: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KucoinWsResponse {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinTickerData {
    symbol: String,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    vol_value: Option<String>,
    #[serde(default)]
    change_price: Option<String>,
    #[serde(default)]
    change_rate: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    sequence: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinTradeData {
    symbol: String,
    trade_id: String,
    price: String,
    size: String,
    side: String,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    taker_order_id: Option<String>,
    #[serde(default)]
    maker_order_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct KucoinCandleData {
    symbol: String,
    candles: Vec<String>,
    #[serde(default)]
    time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(KucoinWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(KucoinWs::format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KucoinWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(KucoinWs::to_unified_symbol("ETH-BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KucoinWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(KucoinWs::format_interval(Timeframe::Hour1), "1hour");
        assert_eq!(KucoinWs::format_interval(Timeframe::Day1), "1day");
    }
}
