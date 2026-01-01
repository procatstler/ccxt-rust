//! Kraken Futures WebSocket Implementation
//!
//! Kraken Futures 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha512, Digest};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Trade, TakerOrMaker,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://futures.kraken.com/ws/v1";

/// Kraken Futures WebSocket 클라이언트
pub struct KrakenFuturesWs {
    config: ExchangeConfig,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    challenge: Arc<RwLock<Option<String>>>,
    signed_challenge: Arc<RwLock<Option<String>>>,
}

/// Kraken Futures ticker data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTicker {
    feed: Option<String>,
    product_id: Option<String>,
    bid: Option<f64>,
    ask: Option<f64>,
    bid_size: Option<f64>,
    ask_size: Option<f64>,
    volume: Option<f64>,
    dtm: Option<i64>,
    leverage: Option<String>,
    index: Option<f64>,
    premium: Option<f64>,
    last: Option<f64>,
    time: Option<i64>,
    change: Option<f64>,
    funding_rate: Option<f64>,
    funding_rate_prediction: Option<f64>,
    suspended: Option<bool>,
    tag: Option<String>,
    pair: Option<String>,
    open_interest: Option<f64>,
    mark_price: Option<f64>,
    maturity_time: Option<i64>,
    post_only: Option<bool>,
    volume_quote: Option<f64>,
}

/// Kraken Futures order book data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderBook {
    feed: Option<String>,
    product_id: Option<String>,
    timestamp: Option<i64>,
    seq: Option<i64>,
    bids: Option<Vec<KfWsOrderBookEntry>>,
    asks: Option<Vec<KfWsOrderBookEntry>>,
    // For updates
    side: Option<String>,
    price: Option<f64>,
    qty: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderBookEntry {
    price: f64,
    qty: f64,
}

/// Kraken Futures trade data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTrade {
    feed: Option<String>,
    product_id: Option<String>,
    uid: Option<String>,
    side: Option<String>,
    #[serde(rename = "type")]
    trade_type: Option<String>,
    seq: Option<i64>,
    time: Option<i64>,
    qty: Option<f64>,
    price: Option<f64>,
}

/// Kraken Futures trade snapshot data
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTradeSnapshot {
    feed: Option<String>,
    product_id: Option<String>,
    trades: Option<Vec<KfWsTrade>>,
}

/// Challenge response for authentication
#[derive(Debug, Deserialize, Serialize)]
struct KfWsChallenge {
    event: Option<String>,
    message: Option<String>,
}

/// Subscription response
#[derive(Debug, Deserialize, Serialize)]
struct KfWsSubscription {
    event: Option<String>,
    feed: Option<String>,
    product_ids: Option<Vec<String>>,
}

impl KrakenFuturesWs {
    /// 새 Kraken Futures WebSocket 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            challenge: Arc::new(RwLock::new(None)),
            signed_challenge: Arc::new(RwLock::new(None)),
        }
    }

    /// 심볼을 Kraken Futures 마켓 ID로 변환
    fn to_market_id(symbol: &str) -> String {
        // BTC/USD:USD -> pi_xbtusd
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return symbol.to_lowercase();
        }

        let base_lower = parts[0].to_lowercase();
        let base = if parts[0] == "BTC" { "xbt".to_string() } else { base_lower };
        let quote_settle: Vec<&str> = parts[1].split(':').collect();

        if quote_settle.len() == 2 {
            let settle_parts: Vec<&str> = quote_settle[1].split('-').collect();
            if settle_parts.len() > 1 {
                format!("fi_{}usd_{}", base, settle_parts[1])
            } else {
                format!("pi_{}usd", base)
            }
        } else {
            format!("pi_{}usd", base)
        }
    }

    /// 마켓 ID를 통합 심볼로 변환
    fn to_unified_symbol(market_id: &str) -> String {
        let lower = market_id.to_lowercase();

        if lower.starts_with("pi_") {
            // pi_xbtusd -> BTC/USD:USD
            let inner = &lower[3..];
            let base = if inner.starts_with("xbt") {
                "BTC"
            } else if inner.starts_with("eth") {
                "ETH"
            } else if inner.starts_with("ltc") {
                "LTC"
            } else if inner.starts_with("xrp") {
                "XRP"
            } else if inner.starts_with("bch") {
                "BCH"
            } else {
                return market_id.to_uppercase();
            };
            format!("{}/USD:USD", base)
        } else if lower.starts_with("fi_") {
            // fi_xbtusd_240329 -> BTC/USD:USD-240329
            let inner = &lower[3..];
            let parts: Vec<&str> = inner.split('_').collect();
            let base = if parts[0].starts_with("xbt") {
                "BTC"
            } else if parts[0].starts_with("eth") {
                "ETH"
            } else {
                return market_id.to_uppercase();
            };
            if parts.len() > 1 {
                format!("{}/USD:USD-{}", base, parts[1].to_uppercase())
            } else {
                format!("{}/USD:USD", base)
            }
        } else if lower.starts_with("pf_") {
            // pf_xbtusd -> BTC/USD:USD (flex futures)
            let inner = &lower[3..];
            let base = if inner.starts_with("xbt") {
                "BTC"
            } else if inner.starts_with("eth") {
                "ETH"
            } else {
                return market_id.to_uppercase();
            };
            format!("{}/USD:USD", base)
        } else {
            market_id.to_uppercase()
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KfWsTicker) -> Option<WsTickerEvent> {
        let product_id = data.product_id.as_ref()?;
        let symbol = Self::to_unified_symbol(product_id);
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.bid.and_then(|v| Decimal::from_f64_retain(v)),
            bid_volume: data.bid_size.and_then(|v| Decimal::from_f64_retain(v)),
            ask: data.ask.and_then(|v| Decimal::from_f64_retain(v)),
            ask_volume: data.ask_size.and_then(|v| Decimal::from_f64_retain(v)),
            vwap: None,
            open: None,
            close: data.last.and_then(|v| Decimal::from_f64_retain(v)),
            last: data.last.and_then(|v| Decimal::from_f64_retain(v)),
            previous_close: None,
            change: data.change.and_then(|v| Decimal::from_f64_retain(v)),
            percentage: None,
            average: None,
            base_volume: data.volume.and_then(|v| Decimal::from_f64_retain(v)),
            quote_volume: data.volume_quote.and_then(|v| Decimal::from_f64_retain(v)),
            index_price: data.index.and_then(|v| Decimal::from_f64_retain(v)),
            mark_price: data.mark_price.and_then(|v| Decimal::from_f64_retain(v)),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTickerEvent { symbol, ticker })
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &KfWsOrderBook, is_snapshot: bool) -> Option<WsOrderBookEvent> {
        let product_id = data.product_id.as_ref()?;
        let symbol = Self::to_unified_symbol(product_id);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.bids.as_ref()
            .map(|entries| {
                entries.iter().filter_map(|e| {
                    Some(OrderBookEntry {
                        price: Decimal::from_f64_retain(e.price)?,
                        amount: Decimal::from_f64_retain(e.qty)?,
                    })
                }).collect()
            })
            .unwrap_or_default();

        let asks: Vec<OrderBookEntry> = data.asks.as_ref()
            .map(|entries| {
                entries.iter().filter_map(|e| {
                    Some(OrderBookEntry {
                        price: Decimal::from_f64_retain(e.price)?,
                        amount: Decimal::from_f64_retain(e.qty)?,
                    })
                }).collect()
            })
            .unwrap_or_default();

        let order_book = OrderBook {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.seq,
            bids,
            asks,
        };

        Some(WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot,
        })
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &KfWsTrade) -> Option<Trade> {
        let product_id = data.product_id.as_ref()?;
        let symbol = Self::to_unified_symbol(product_id);
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = Decimal::from_f64_retain(data.price?)?;
        let amount = Decimal::from_f64_retain(data.qty?)?;

        Some(Trade {
            id: data.uid.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol,
            trade_type: data.trade_type.clone(),
            side: data.side.clone(),
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 체결 스냅샷 파싱
    fn parse_trade_snapshot(data: &KfWsTradeSnapshot) -> Option<WsTradeEvent> {
        let product_id = data.product_id.as_ref()?;
        let symbol = Self::to_unified_symbol(product_id);

        let trades: Vec<Trade> = data.trades.as_ref()
            .map(|trades| {
                trades.iter()
                    .filter_map(|t| Self::parse_trade(t))
                    .collect()
            })
            .unwrap_or_default();

        Some(WsTradeEvent { symbol, trades })
    }

    /// Sign challenge for authentication
    fn sign_challenge(&self, challenge: &str) -> Option<String> {
        let secret = self.config.secret()?;

        // Step 1: SHA256 hash of challenge
        let mut hasher = Sha256::new();
        hasher.update(challenge.as_bytes());
        let hash = hasher.finalize();

        // Step 2: Decode secret from base64
        let secret_decoded = BASE64.decode(secret).ok()?;

        // Step 3: HMAC-SHA512
        let mut mac = Hmac::<Sha512>::new_from_slice(&secret_decoded).ok()?;
        mac.update(&hash);
        let signature = BASE64.encode(mac.finalize().into_bytes());

        Some(signature)
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        let value: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Check feed type
        let feed = value.get("feed").and_then(|v| v.as_str())?;

        match feed {
            "ticker" | "ticker_lite" => {
                if let Ok(ticker_data) = serde_json::from_value::<KfWsTicker>(value) {
                    if let Some(event) = Self::parse_ticker(&ticker_data) {
                        return Some(WsMessage::Ticker(event));
                    }
                }
            }
            "book_snapshot" => {
                if let Ok(ob_data) = serde_json::from_value::<KfWsOrderBook>(value) {
                    if let Some(event) = Self::parse_order_book(&ob_data, true) {
                        return Some(WsMessage::OrderBook(event));
                    }
                }
            }
            "book" => {
                if let Ok(ob_data) = serde_json::from_value::<KfWsOrderBook>(value) {
                    if let Some(event) = Self::parse_order_book(&ob_data, false) {
                        return Some(WsMessage::OrderBook(event));
                    }
                }
            }
            "trade" => {
                if let Ok(trade_data) = serde_json::from_value::<KfWsTrade>(value) {
                    if let Some(trade) = Self::parse_trade(&trade_data) {
                        let symbol = trade.symbol.clone();
                        return Some(WsMessage::Trade(WsTradeEvent {
                            symbol,
                            trades: vec![trade],
                        }));
                    }
                }
            }
            "trade_snapshot" => {
                if let Ok(snapshot_data) = serde_json::from_value::<KfWsTradeSnapshot>(value) {
                    if let Some(event) = Self::parse_trade_snapshot(&snapshot_data) {
                        return Some(WsMessage::Trade(event));
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
        product_ids: Vec<String>,
        feed: &str,
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
        let subscribe_msg = serde_json::json!({
            "event": "subscribe",
            "feed": feed,
            "product_ids": product_ids
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", feed, product_ids.join(","));
            self.subscriptions.write().await.insert(key, feed.to_string());
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

impl Default for KrakenFuturesWs {
    fn default() -> Self {
        Self::new(ExchangeConfig::default())
    }
}

#[async_trait]
impl WsExchange for KrakenFuturesWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        client.subscribe_stream(vec![market_id], "ticker").await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_ids: Vec<String> = symbols.iter()
            .map(|s| Self::to_market_id(s))
            .collect();
        client.subscribe_stream(market_ids, "ticker").await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        client.subscribe_stream(vec![market_id], "book").await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        client.subscribe_stream(vec![market_id], "trade").await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: crate::types::Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Kraken Futures does not support OHLCV WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOHLCV".to_string(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ref ws_client) = self.ws_client {
            ws_client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(ref ws_client) = self.ws_client {
            ws_client.is_connected().await
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_to_market_id() {
        assert_eq!(KrakenFuturesWs::to_market_id("BTC/USD:USD"), "pi_xbtusd");
        assert_eq!(KrakenFuturesWs::to_market_id("ETH/USD:USD"), "pi_ethusd");
        assert_eq!(KrakenFuturesWs::to_market_id("BTC/USD:USD-240329"), "fi_xbtusd_240329");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KrakenFuturesWs::to_unified_symbol("PI_XBTUSD"), "BTC/USD:USD");
        assert_eq!(KrakenFuturesWs::to_unified_symbol("PI_ETHUSD"), "ETH/USD:USD");
        assert_eq!(KrakenFuturesWs::to_unified_symbol("FI_XBTUSD_240329"), "BTC/USD:USD-240329");
        assert_eq!(KrakenFuturesWs::to_unified_symbol("PF_XBTUSD"), "BTC/USD:USD");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = KfWsTicker {
            feed: Some("ticker".to_string()),
            product_id: Some("PI_XBTUSD".to_string()),
            bid: Some(42000.0),
            ask: Some(42001.0),
            bid_size: Some(10.0),
            ask_size: Some(5.0),
            volume: Some(1000.0),
            dtm: None,
            leverage: None,
            index: Some(42000.5),
            premium: None,
            last: Some(42000.5),
            time: Some(1609459200000),
            change: Some(1.5),
            funding_rate: None,
            funding_rate_prediction: None,
            suspended: None,
            tag: None,
            pair: None,
            open_interest: None,
            mark_price: Some(42000.0),
            maturity_time: None,
            post_only: None,
            volume_quote: None,
        };

        let event = KrakenFuturesWs::parse_ticker(&ticker_data).unwrap();
        assert_eq!(event.symbol, "BTC/USD:USD");
        assert_eq!(event.ticker.bid, Some(Decimal::from_str("42000").unwrap()));
        assert_eq!(event.ticker.ask, Some(Decimal::from_str("42001").unwrap()));
    }

    #[test]
    fn test_parse_trade() {
        let trade_data = KfWsTrade {
            feed: Some("trade".to_string()),
            product_id: Some("PI_XBTUSD".to_string()),
            uid: Some("abc123".to_string()),
            side: Some("buy".to_string()),
            trade_type: Some("fill".to_string()),
            seq: Some(12345),
            time: Some(1609459200000),
            qty: Some(1.5),
            price: Some(42000.0),
        };

        let trade = KrakenFuturesWs::parse_trade(&trade_data).unwrap();
        assert_eq!(trade.symbol, "BTC/USD:USD");
        assert_eq!(trade.id, "abc123");
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.price, Decimal::from_str("42000").unwrap());
    }
}
