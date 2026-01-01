//! Coinbase Exchange WebSocket Implementation
//!
//! Coinbase Exchange (formerly GDAX) real-time data streaming

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
    OrderBook, OrderBookEntry, TakerOrMaker, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws-feed.exchange.coinbase.com";

/// Coinbase Exchange WebSocket 클라이언트
///
/// Coinbase Exchange (formerly GDAX)는 기관 거래자를 위한 거래소입니다.
/// - WebSocket URL: wss://ws-feed.exchange.coinbase.com
/// - 채널: ticker, level2, matches, full
pub struct CoinbaseExchangeWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl CoinbaseExchangeWs {
    /// 새 Coinbase Exchange WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 심볼을 Coinbase 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Coinbase 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(coinbase_symbol: &str) -> String {
        coinbase_symbol.replace("-", "/")
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinbaseExchangeTickerMessage) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.time
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.time.clone(),
            high: data.high_24h,
            low: data.low_24h,
            bid: data.best_bid,
            bid_volume: data.best_bid_size,
            ask: data.best_ask,
            ask_volume: data.best_ask_size,
            vwap: None,
            open: data.open_24h,
            close: data.price,
            last: data.price,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_24h,
            quote_volume: data.volume_30d,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book_snapshot(data: &CoinbaseExchangeSnapshotMessage) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data.bids.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: entry[0],
                        amount: entry[1],
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: entry[0],
                        amount: entry[1],
                    })
                } else {
                    None
                }
            })
            .collect();

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
            is_snapshot: true,
        }
    }

    /// L2 업데이트 파싱
    fn parse_l2_update(data: &CoinbaseExchangeL2UpdateMessage, cache: &mut OrderBook) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.time
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // 업데이트 적용
        for change in &data.changes {
            if change.len() >= 3 {
                let side = &change[0];
                if let (Ok(price), Ok(size)) = (
                    change[1].parse::<Decimal>(),
                    change[2].parse::<Decimal>()
                ) {
                    let entries = if side == "buy" {
                        &mut cache.bids
                    } else {
                        &mut cache.asks
                    };

                    // 기존 가격 레벨 업데이트 또는 제거
                    entries.retain(|e| e.price != price);
                    if size > Decimal::ZERO {
                        entries.push(OrderBookEntry { price, amount: size });
                    }

                    // 정렬
                    if side == "buy" {
                        entries.sort_by(|a, b| b.price.cmp(&a.price));
                    } else {
                        entries.sort_by(|a, b| a.price.cmp(&b.price));
                    }
                }
            }
        }

        cache.timestamp = Some(timestamp);
        cache.datetime = Some(Utc::now().to_rfc3339());

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book: cache.clone(),
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_match(data: &CoinbaseExchangeMatchMessage) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.time
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let trade = Trade {
            id: data.trade_id.map(|id| id.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: data.time.clone(),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: Some(TakerOrMaker::Taker),
            price: data.price.unwrap_or_default(),
            amount: data.size.unwrap_or_default(),
            cost: data.price.and_then(|p| data.size.map(|s| p * s)),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, order_book_cache: &mut HashMap<String, OrderBook>) -> Option<WsMessage> {
        if let Ok(base) = serde_json::from_str::<CoinbaseExchangeBaseMessage>(msg) {
            match base.message_type.as_deref() {
                Some("subscriptions") => {
                    return Some(WsMessage::Subscribed {
                        channel: "subscriptions".to_string(),
                        symbol: None,
                    });
                }
                Some("ticker") => {
                    if let Ok(ticker_msg) = serde_json::from_str::<CoinbaseExchangeTickerMessage>(msg) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_msg)));
                    }
                }
                Some("snapshot") => {
                    if let Ok(snapshot_msg) = serde_json::from_str::<CoinbaseExchangeSnapshotMessage>(msg) {
                        let event = Self::parse_order_book_snapshot(&snapshot_msg);
                        // 캐시 초기화
                        order_book_cache.insert(event.symbol.clone(), event.order_book.clone());
                        return Some(WsMessage::OrderBook(event));
                    }
                }
                Some("l2update") => {
                    if let Ok(update_msg) = serde_json::from_str::<CoinbaseExchangeL2UpdateMessage>(msg) {
                        let unified_symbol = Self::to_unified_symbol(&update_msg.product_id);
                        if let Some(cache) = order_book_cache.get_mut(&unified_symbol) {
                            let event = Self::parse_l2_update(&update_msg, cache);
                            return Some(WsMessage::OrderBook(event));
                        }
                    }
                }
                Some("match") | Some("last_match") => {
                    if let Ok(match_msg) = serde_json::from_str::<CoinbaseExchangeMatchMessage>(msg) {
                        return Some(WsMessage::Trade(Self::parse_match(&match_msg)));
                    }
                }
                Some("error") => {
                    if let Some(message) = base.message {
                        return Some(WsMessage::Error(message));
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        product_ids: Vec<String>,
        channels: Vec<String>,
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
        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "product_ids": product_ids,
            "channels": channels
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channels.join(","), product_ids.join(","));
            self.subscriptions.write().await.insert(key, channels.join(","));
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let order_book_cache = Arc::clone(&self.order_book_cache);

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
                        let mut cache = order_book_cache.write().await;
                        if let Some(ws_msg) = Self::process_message(&msg, &mut cache) {
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

impl Default for CoinbaseExchangeWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinbaseExchangeWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], vec!["ticker".to_string()]).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(coinbase_symbols, vec!["ticker".to_string()]).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], vec!["level2".to_string()]).await
    }

    async fn watch_order_book_for_symbols(&self, symbols: &[&str], _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(coinbase_symbols, vec!["level2".to_string()]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], vec!["matches".to_string()]).await
    }

    async fn watch_trades_for_symbols(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(coinbase_symbols, vec!["matches".to_string()]).await
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "Coinbase Exchange WebSocket does not support OHLCV streaming".into(),
        })
    }

    async fn watch_ohlcv_for_symbols(&self, _symbols: &[&str], _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "Coinbase Exchange WebSocket does not support OHLCV streaming".into(),
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
}

// === Coinbase Exchange WebSocket Types ===

#[derive(Debug, Deserialize)]
struct CoinbaseExchangeBaseMessage {
    #[serde(default, rename = "type")]
    message_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeTickerMessage {
    #[serde(default, rename = "type")]
    message_type: Option<String>,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    open_24h: Option<Decimal>,
    #[serde(default)]
    volume_24h: Option<Decimal>,
    #[serde(default)]
    volume_30d: Option<Decimal>,
    #[serde(default)]
    low_24h: Option<Decimal>,
    #[serde(default)]
    high_24h: Option<Decimal>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_bid_size: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    best_ask_size: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    last_size: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseExchangeSnapshotMessage {
    #[serde(default, rename = "type")]
    message_type: Option<String>,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseExchangeL2UpdateMessage {
    #[serde(default, rename = "type")]
    message_type: Option<String>,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    changes: Vec<Vec<String>>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeMatchMessage {
    #[serde(default, rename = "type")]
    message_type: Option<String>,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    sequence: Option<i64>,
    #[serde(default)]
    maker_order_id: Option<String>,
    #[serde(default)]
    taker_order_id: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coinbaseexchange_ws_creation() {
        let _ws = CoinbaseExchangeWs::new();
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinbaseExchangeWs::format_symbol("BTC/USD"), "BTC-USD");
        assert_eq!(CoinbaseExchangeWs::format_symbol("ETH/EUR"), "ETH-EUR");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinbaseExchangeWs::to_unified_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(CoinbaseExchangeWs::to_unified_symbol("ETH-EUR"), "ETH/EUR");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_msg = CoinbaseExchangeTickerMessage {
            product_id: "BTC-USD".to_string(),
            price: Some(Decimal::from(50000)),
            volume_24h: Some(Decimal::from(1000)),
            high_24h: Some(Decimal::from(51000)),
            low_24h: Some(Decimal::from(49000)),
            best_bid: Some(Decimal::from(49990)),
            best_ask: Some(Decimal::from(50010)),
            ..Default::default()
        };

        let result = CoinbaseExchangeWs::parse_ticker(&ticker_msg);
        assert_eq!(result.symbol, "BTC/USD");
        assert_eq!(result.ticker.last, Some(Decimal::from(50000)));
    }
}
