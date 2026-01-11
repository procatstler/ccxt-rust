//! Korbit WebSocket Implementation
//!
//! Korbit 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws.korbit.co.kr/v1/user/push";

/// Korbit WebSocket 클라이언트
pub struct KorbitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl KorbitWs {
    /// 새 Korbit WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
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
            api_key: Some(api_key),
            api_secret: Some(api_secret),
        }
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.api_key = Some(api_key);
        self.api_secret = Some(api_secret);
    }

    /// 심볼을 Korbit 형식으로 변환 (BTC/KRW -> btc_krw)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// Korbit 심볼을 통합 심볼로 변환 (btc_krw -> BTC/KRW)
    fn to_unified_symbol(korbit_symbol: &str) -> String {
        let parts: Vec<&str> = korbit_symbol.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            korbit_symbol.to_uppercase()
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KorbitWsTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_percent.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &KorbitWsOrderBookData) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &KorbitWsTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.amount.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let trades = vec![Trade {
            id: data.tid.map(|id| id.to_string()).unwrap_or_default(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            order: None,
            trade_type: None,
            side: data.r#type.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str, _subscribed_symbol: Option<&str>) -> Option<WsMessage> {
        // Ping/Pong 처리
        if msg.contains("ping") || msg.contains("pong") {
            return None;
        }

        let response: KorbitWsResponse = serde_json::from_str(msg).ok()?;

        let channel = response.event.as_deref()?;

        match channel {
            "korbit:ticker" => {
                if let Some(data) = response.data {
                    if let Ok(ticker_data) = serde_json::from_value::<KorbitWsTickerData>(data) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }
            }
            "korbit:orderbook" => {
                if let Some(data) = response.data {
                    if let Ok(book_data) = serde_json::from_value::<KorbitWsOrderBookData>(data) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data)));
                    }
                }
            }
            "korbit:transaction" => {
                if let Some(data) = response.data {
                    if let Ok(trade_data) = serde_json::from_value::<KorbitWsTradeData>(data) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                    }
                }
            }
            _ => {}
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        subscribe_msg: serde_json::Value,
        channel: &str,
        symbol: Option<&str>,
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

        // 구독 메시지 전송
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let subscribed_symbol = symbol.map(|s| s.to_string());
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
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref()) {
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

impl Default for KorbitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for KorbitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "accessToken": null,
            "timestamp": Utc::now().timestamp_millis(),
            "event": "korbit:subscribe",
            "data": {
                "channels": [format!("ticker:{}", formatted)]
            }
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channels: Vec<String> = symbols.iter()
            .map(|s| format!("ticker:{}", Self::format_symbol(s)))
            .collect();
        let subscribe_msg = serde_json::json!({
            "accessToken": null,
            "timestamp": Utc::now().timestamp_millis(),
            "event": "korbit:subscribe",
            "data": {
                "channels": channels
            }
        });
        client.subscribe_stream(subscribe_msg, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "accessToken": null,
            "timestamp": Utc::now().timestamp_millis(),
            "event": "korbit:subscribe",
            "data": {
                "channels": [format!("orderbook:{}", formatted)]
            }
        });
        client.subscribe_stream(subscribe_msg, "orderbook", Some(&formatted)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "accessToken": null,
            "timestamp": Utc::now().timestamp_millis(),
            "event": "korbit:subscribe",
            "data": {
                "channels": [format!("transaction:{}", formatted)]
            }
        });
        client.subscribe_stream(subscribe_msg, "transaction", Some(&formatted)).await
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Korbit doesn't support OHLCV via WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOhlcv".into(),
        })
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

    // === Private Channel Methods ===

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(crate::errors::CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_orders - Private WebSocket not implemented yet".to_string(),
        })
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(crate::errors::CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_my_trades - Private WebSocket not implemented yet".to_string(),
        })
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(crate::errors::CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_balance - Private WebSocket not implemented yet".to_string(),
        })
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct KorbitWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitWsTickerData {
    #[serde(default)]
    currency_pair: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default, rename = "changePercent")]
    change_percent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitWsOrderBookData {
    #[serde(default)]
    currency_pair: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitWsTradeData {
    #[serde(default)]
    currency_pair: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    tid: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    r#type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(KorbitWs::format_symbol("BTC/KRW"), "btc_krw");
        assert_eq!(KorbitWs::format_symbol("ETH/KRW"), "eth_krw");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KorbitWs::to_unified_symbol("btc_krw"), "BTC/KRW");
        assert_eq!(KorbitWs::to_unified_symbol("eth_krw"), "ETH/KRW");
    }

    #[test]
    fn test_with_credentials() {
        let ws = KorbitWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = KorbitWs::new();
        assert!(ws.api_key.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }
}
