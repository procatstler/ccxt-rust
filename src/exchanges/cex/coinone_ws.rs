//! Coinone WebSocket Implementation
//!
//! Coinone 실시간 데이터 스트리밍

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

const WS_PUBLIC_URL: &str = "wss://stream.coinone.co.kr";

/// Coinone WebSocket 클라이언트
pub struct CoinoneWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    api_key: Option<String>,
    api_secret: Option<String>,
}

impl CoinoneWs {
    /// 새 Coinone WebSocket 클라이언트 생성
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

    /// 심볼을 Coinone 형식으로 변환 (BTC/KRW -> btc-krw)
    fn format_symbol(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            format!("{}-{}", parts[0].to_lowercase(), parts[1].to_lowercase())
        } else {
            symbol.to_lowercase()
        }
    }

    /// Coinone 심볼을 통합 심볼로 변환 (btc-krw -> BTC/KRW)
    fn to_unified_symbol(coinone_symbol: &str) -> String {
        let parts: Vec<&str> = coinone_symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            coinone_symbol.to_uppercase()
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinoneTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.target_currency_pair);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.best_bid,
            bid_volume: None,
            ask: data.best_ask,
            ask_volume: None,
            vwap: None,
            open: data.first,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.change_price,
            percentage: data.change_rate.map(|r| r * Decimal::from(100)),
            average: None,
            base_volume: data.target_volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &CoinoneOrderBookData) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.target_currency_pair);

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                let price = b.price.parse::<Decimal>().ok()?;
                let amount = b.qty.parse::<Decimal>().ok()?;
                Some(OrderBookEntry { price, amount })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                let price = a.price.parse::<Decimal>().ok()?;
                let amount = a.qty.parse::<Decimal>().ok()?;
                Some(OrderBookEntry { price, amount })
            })
            .collect();

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

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
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &CoinoneTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.target_currency_pair);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.qty.parse().unwrap_or_default();

        let side = if data.is_seller_maker.unwrap_or(false) {
            "sell"
        } else {
            "buy"
        };

        let trades = vec![Trade {
            id: data.id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Try to parse as JSON
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Check response type
        let response_type = json.get("response_type").and_then(|v| v.as_str())?;

        match response_type {
            "DATA" => {
                let channel = json.get("channel").and_then(|v| v.as_str())?;
                let data = json.get("data")?;

                match channel {
                    "TICKER" => {
                        let ticker_data: CoinoneTickerData =
                            serde_json::from_value(data.clone()).ok()?;
                        Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)))
                    },
                    "ORDERBOOK" => {
                        let ob_data: CoinoneOrderBookData =
                            serde_json::from_value(data.clone()).ok()?;
                        Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data)))
                    },
                    "TRADE" => {
                        let trade_data: CoinoneTradeData =
                            serde_json::from_value(data.clone()).ok()?;
                        Some(WsMessage::Trade(Self::parse_trade(&trade_data)))
                    },
                    _ => None,
                }
            },
            "SUBSCRIBED" | "UNSUBSCRIBED" | "PONG" => None,
            _ => None,
        }
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbols: Vec<String>,
        subscription_key: &str,
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
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "request_type": "SUBSCRIBE",
            "channel": channel,
            "topic": {
                "quote_currency": "KRW",
                "target_currency": symbols
            }
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", subscription_key, symbol.unwrap_or(""));
            self.subscriptions
                .write()
                .await
                .insert(key, subscription_key.to_string());
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

    /// 심볼에서 타겟 통화 추출 (BTC/KRW -> BTC)
    fn extract_target_currency(symbol: &str) -> String {
        symbol.split('/').next().unwrap_or(symbol).to_uppercase()
    }
}

impl Default for CoinoneWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinoneWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let target = Self::extract_target_currency(symbol);
        client
            .subscribe_stream("TICKER", vec![target], "ticker", Some(symbol))
            .await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let targets: Vec<String> = symbols
            .iter()
            .map(|s| Self::extract_target_currency(s))
            .collect();
        client
            .subscribe_stream("TICKER", targets, "tickers", None)
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let target = Self::extract_target_currency(symbol);
        client
            .subscribe_stream("ORDERBOOK", vec![target], "orderBook", Some(symbol))
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let target = Self::extract_target_currency(symbol);
        client
            .subscribe_stream("TRADE", vec![target], "trades", Some(symbol))
            .await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coinone does not support OHLCV streaming via WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
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

    async fn watch_orders(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        if self.api_key.is_none() || self.api_secret.is_none() {
            return Err(crate::errors::CcxtError::AuthenticationError {
                message: "API credentials required for private channels".into(),
            });
        }
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_orders - Private WebSocket not implemented yet".to_string(),
        })
    }

    async fn watch_my_trades(
        &self,
        _symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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

// === Coinone WebSocket Types ===

#[derive(Debug, Deserialize, Serialize)]
struct CoinoneTickerData {
    #[serde(default)]
    target_currency_pair: String,
    #[serde(default)]
    quote_currency: Option<String>,
    #[serde(default)]
    target_currency: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    first: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    target_volume: Option<Decimal>,
    #[serde(default)]
    quote_volume: Option<Decimal>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    change_price: Option<Decimal>,
    #[serde(default)]
    change_rate: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinoneOrderBookData {
    #[serde(default)]
    target_currency_pair: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<CoinoneOrderBookEntry>,
    #[serde(default)]
    asks: Vec<CoinoneOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinoneOrderBookEntry {
    #[serde(default)]
    price: String,
    #[serde(default)]
    qty: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinoneTradeData {
    #[serde(default)]
    target_currency_pair: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    price: String,
    #[serde(default)]
    qty: String,
    #[serde(default)]
    is_seller_maker: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinoneWs::format_symbol("BTC/KRW"), "btc-krw");
        assert_eq!(CoinoneWs::format_symbol("ETH/KRW"), "eth-krw");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinoneWs::to_unified_symbol("btc-krw"), "BTC/KRW");
        assert_eq!(CoinoneWs::to_unified_symbol("eth-krw"), "ETH/KRW");
    }

    #[test]
    fn test_extract_target_currency() {
        assert_eq!(CoinoneWs::extract_target_currency("BTC/KRW"), "BTC");
        assert_eq!(CoinoneWs::extract_target_currency("ETH/KRW"), "ETH");
    }

    #[test]
    fn test_with_credentials() {
        let ws = CoinoneWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = CoinoneWs::new();
        assert!(ws.api_key.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.api_key.is_some());
        assert!(ws.api_secret.is_some());
    }
}
