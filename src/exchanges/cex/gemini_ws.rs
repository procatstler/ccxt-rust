//! Gemini WebSocket Implementation
//!
//! Gemini 실시간 데이터 스트리밍

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOhlcvEvent,
    WsOrderBookEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_BASE_URL: &str = "wss://api.gemini.com/v2/marketdata";
const WS_SANDBOX_URL: &str = "wss://api.sandbox.gemini.com/v2/marketdata";

/// Gemini WebSocket 클라이언트
pub struct GeminiWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    sandbox: bool,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl GeminiWs {
    /// 새 Gemini WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            sandbox: false,
            api_key: None,
            api_secret: None,
        }
    }

    /// Sandbox mode로 생성
    pub fn new_sandbox() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            sandbox: true,
            api_key: None,
            api_secret: None,
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            sandbox: false,
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// 심볼을 Gemini 형식으로 변환 (BTC/USD -> BTCUSD)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_uppercase()
    }

    /// Gemini 심볼을 통합 심볼로 변환 (BTCUSD -> BTC/USD)
    fn to_unified_symbol(gemini_symbol: &str) -> String {
        let symbol = gemini_symbol.to_uppercase();

        // Common quote currencies
        let quotes = ["USDT", "USD", "BTC", "ETH", "GUSD", "DAI"];

        for quote in quotes {
            if let Some(base) = symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        // Default: return as is
        gemini_symbol.to_string()
    }

    /// Timeframe을 Gemini candles interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour6 => "6h",
            Timeframe::Day1 => "1d",
            _ => "1m", // Default to 1m for unsupported timeframes
        }
    }

    /// 티커 메시지 파싱 (l2 updates에서 추출)
    fn parse_ticker_from_l2(data: &GeminiL2Update) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // Extract bid/ask from changes
        let mut bid: Option<Decimal> = None;
        let mut ask: Option<Decimal> = None;

        for change in &data.changes {
            if change.len() >= 3 {
                let side = &change[0];
                let price: Option<Decimal> = change[1].parse().ok();

                if side == "buy" && bid.is_none() {
                    bid = price;
                } else if side == "sell" && ask.is_none() {
                    ask = price;
                }
            }
        }

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid,
            bid_volume: None,
            ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: None,
            last: None,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &GeminiL2Update) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for change in &data.changes {
            if change.len() >= 3 {
                let side = &change[0];
                let price: Decimal = change[1].parse().unwrap_or_default();
                let amount: Decimal = change[2].parse().unwrap_or_default();

                if amount > Decimal::ZERO {
                    let entry = OrderBookEntry { price, amount };
                    if side == "buy" {
                        bids.push(entry);
                    } else if side == "sell" {
                        asks.push(entry);
                    }
                }
            }
        }

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
            checksum: None,
        };

        WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trades(data: &GeminiL2Update) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);

        let trades: Vec<Trade> = data
            .trades
            .iter()
            .map(|t| {
                let timestamp = t.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.quantity.parse().unwrap_or_default();

                Trade {
                    id: t.event_id.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side: Some(t.side.clone()),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        WsTradeEvent { symbol, trades }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &GeminiCandleUpdate, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);

        // Take the first candle from changes array
        if let Some(candle) = data.changes.first() {
            if candle.len() >= 6 {
                let ohlcv = OHLCV {
                    timestamp: candle[0]
                        .parse()
                        .unwrap_or_else(|_| Utc::now().timestamp_millis()),
                    open: candle[1].parse().unwrap_or_default(),
                    high: candle[2].parse().unwrap_or_default(),
                    low: candle[3].parse().unwrap_or_default(),
                    close: candle[4].parse().unwrap_or_default(),
                    volume: candle[5].parse().unwrap_or_default(),
                };

                return WsOhlcvEvent {
                    symbol,
                    timeframe,
                    ohlcv,
                };
            }
        }

        // Return empty OHLCV if parsing fails
        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv: OHLCV {
                timestamp: Utc::now().timestamp_millis(),
                open: Decimal::ZERO,
                high: Decimal::ZERO,
                low: Decimal::ZERO,
                close: Decimal::ZERO,
                volume: Decimal::ZERO,
            },
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse JSON
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;
        let msg_type = json.get("type").and_then(|v| v.as_str())?;

        match msg_type {
            "l2_updates" => {
                if let Ok(data) = serde_json::from_str::<GeminiL2Update>(msg) {
                    // Check if there are trades
                    if !data.trades.is_empty() {
                        return Some(WsMessage::Trade(Self::parse_trades(&data)));
                    }
                    // Otherwise return orderbook update
                    return Some(WsMessage::OrderBook(Self::parse_order_book(&data)));
                }
            },
            s if s.starts_with("candles_") && s.ends_with("_updates") => {
                if let Ok(data) = serde_json::from_str::<GeminiCandleUpdate>(msg) {
                    // Extract timeframe from type (e.g., "candles_1m_updates" -> "1m")
                    let timeframe_str = s
                        .trim_start_matches("candles_")
                        .trim_end_matches("_updates");
                    let timeframe = match timeframe_str {
                        "1m" => Timeframe::Minute1,
                        "5m" => Timeframe::Minute5,
                        "15m" => Timeframe::Minute15,
                        "30m" => Timeframe::Minute30,
                        "1h" => Timeframe::Hour1,
                        "6h" => Timeframe::Hour6,
                        "1d" => Timeframe::Day1,
                        _ => Timeframe::Minute1,
                    };
                    return Some(WsMessage::Ohlcv(Self::parse_ohlcv(&data, timeframe)));
                }
            },
            "subscription_ack" => {
                return Some(WsMessage::Subscribed {
                    channel: "gemini".to_string(),
                    symbol: None,
                });
            },
            "heartbeat" => {
                // Ignore heartbeat messages
                return None;
            },
            _ => {},
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        subscription: GeminiSubscription,
        message_hash: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket URL
        let url = if self.sandbox {
            WS_SANDBOX_URL
        } else {
            WS_BASE_URL
        };

        // Create subscription message
        let subscribe_msg = GeminiSubscribeRequest {
            msg_type: "subscribe".to_string(),
            subscriptions: vec![subscription],
        };

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send subscription message
        let sub_json =
            serde_json::to_string(&subscribe_msg).map_err(|e| CcxtError::ParseError {
                data_type: "GeminiSubscribeRequest".to_string(),
                message: e.to_string(),
            })?;

        // Send subscribe message through WebSocket
        ws_client.send(&sub_json)?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            self.subscriptions.write().await.insert(
                message_hash.to_string(),
                serde_json::to_string(&subscribe_msg).unwrap_or_default(),
            );
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    },
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg) {
                            let _ = tx.send(ws_msg);
                        }
                    },
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for GeminiWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for GeminiWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::clone(&self.order_book_cache),
            sandbox: self.sandbox,
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        }
    }
}

#[async_trait]
impl WsExchange for GeminiWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let gemini_symbol = Self::format_symbol(symbol);

        let subscription = GeminiSubscription {
            name: "l2".to_string(),
            symbols: vec![gemini_symbol.clone()],
        };

        let message_hash = format!("ticker:{symbol}");
        client.subscribe_stream(subscription, &message_hash).await
    }

    async fn watch_tickers(
        &self,
        _symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchTickers".to_string(),
        })
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let gemini_symbol = Self::format_symbol(symbol);

        let subscription = GeminiSubscription {
            name: "l2".to_string(),
            symbols: vec![gemini_symbol.clone()],
        };

        let message_hash = format!("orderbook:{symbol}");
        client.subscribe_stream(subscription, &message_hash).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let gemini_symbol = Self::format_symbol(symbol);

        let subscription = GeminiSubscription {
            name: "l2".to_string(),
            symbols: vec![gemini_symbol.clone()],
        };

        let message_hash = format!("trades:{symbol}");
        client.subscribe_stream(subscription, &message_hash).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();
        let gemini_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);

        let subscription = GeminiSubscription {
            name: format!("candles_{interval}"),
            symbols: vec![gemini_symbol.clone()],
        };

        let message_hash = format!("ohlcv:{symbol}:{interval}");
        client.subscribe_stream(subscription, &message_hash).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchBalance".to_string(),
        })
    }

    async fn watch_orders(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchOrders".to_string(),
        })
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watchMyTrades".to_string(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = self.ws_client.take() {
            let _ = client.close();
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        self.ws_client.is_some()
    }
}

// WebSocket Message Structures

#[derive(Debug, Deserialize, Serialize)]
struct GeminiSubscribeRequest {
    #[serde(rename = "type")]
    msg_type: String,
    subscriptions: Vec<GeminiSubscription>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiSubscription {
    name: String,
    symbols: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiL2Update {
    #[serde(rename = "type")]
    msg_type: String,
    symbol: String,
    changes: Vec<Vec<String>>,
    #[serde(default)]
    trades: Vec<GeminiTrade>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiTrade {
    #[serde(rename = "type")]
    msg_type: String,
    symbol: String,
    event_id: i64,
    timestamp: Option<i64>,
    price: String,
    quantity: String,
    side: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GeminiCandleUpdate {
    #[serde(rename = "type")]
    msg_type: String,
    symbol: String,
    changes: Vec<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let ws = GeminiWs::new();
        assert!(ws.ws_client.is_none());
        assert!(!ws.sandbox);
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_new_sandbox() {
        let ws = GeminiWs::new_sandbox();
        assert!(ws.ws_client.is_none());
        assert!(ws.sandbox);
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
    }

    #[test]
    fn test_with_credentials() {
        let ws = GeminiWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(!ws.sandbox);
        assert_eq!(ws.api_key, Some("test_key".to_string()));
        assert_eq!(ws.api_secret, Some("test_secret".to_string()));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = GeminiWs::new();
        assert!(ws.api_key.is_none());
        assert!(ws.api_secret.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert_eq!(ws.api_key, Some("test_key".to_string()));
        assert_eq!(ws.api_secret, Some("test_secret".to_string()));
    }

    #[test]
    fn test_clone() {
        let ws = GeminiWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        let cloned = ws.clone();
        assert_eq!(cloned.api_key, Some("test_key".to_string()));
        assert_eq!(cloned.api_secret, Some("test_secret".to_string()));
        assert_eq!(cloned.sandbox, ws.sandbox);
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(GeminiWs::format_symbol("BTC/USD"), "BTCUSD");
        assert_eq!(GeminiWs::format_symbol("ETH/BTC"), "ETHBTC");
    }
}
