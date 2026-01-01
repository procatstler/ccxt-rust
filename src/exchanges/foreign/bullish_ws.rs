//! Bullish WebSocket Implementation
//!
//! Bullish 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::{mpsc, RwLock};

use crate::client::ExchangeConfig;
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://api.exchange.bullish.com";

/// Bullish WebSocket 클라이언트
pub struct BullishWs {
    config: ExchangeConfig,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
    _connected: Arc<RwLock<bool>>,
    request_id: AtomicI64,
}

impl BullishWs {
    /// 새 Bullish WebSocket 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            order_books: Arc::new(RwLock::new(HashMap::new())),
            _connected: Arc::new(RwLock::new(false)),
            request_id: AtomicI64::new(0),
        }
    }

    /// 요청 ID 생성
    fn next_request_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 심볼을 Bullish WebSocket 형식으로 변환 (BTC/USDC -> BTCUSDC)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace('/', "")
    }

    /// Bullish 심볼을 통합 심볼로 변환 (BTCUSDC -> BTC/USDC)
    fn to_unified_symbol(bullish_symbol: &str) -> String {
        // Common quote currencies for Bullish
        let quote_currencies = ["USDC", "USDT", "USD", "BTC", "ETH"];

        for quote in &quote_currencies {
            if bullish_symbol.ends_with(quote) {
                let base = &bullish_symbol[..bullish_symbol.len() - quote.len()];
                if !base.is_empty() {
                    return format!("{}/{}", base, quote);
                }
            }
        }

        // Fallback: assume 3-char base + rest is quote
        if bullish_symbol.len() > 3 {
            let (base, quote) = bullish_symbol.split_at(3);
            return format!("{}/{}", base, quote);
        }

        bullish_symbol.to_string()
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BullishWsTickerData) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.created_at_timestamp
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.created_at_datetime.clone().or_else(|| {
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
            }),
            high: data.high.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            low: data.low.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid: data.best_bid.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: data.bid_volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            ask: data.best_ask.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: data.ask_volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            vwap: data.vwap.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            open: data.open.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            close: data.close.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            last: data.last.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            percentage: data.percentage.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            average: data.average.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            base_volume: data.base_volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: data.mark_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BullishWsOrderBookData) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // Parse bids and asks from flat array format [price, amount, price, amount, ...]
        let parse_entries = |entries: &[String]| -> Vec<OrderBookEntry> {
            entries
                .chunks(2)
                .filter_map(|chunk| {
                    if chunk.len() >= 2 {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&chunk[0]).ok()?,
                            amount: Decimal::from_str(&chunk[1]).ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

        // Get nonce from sequence number range
        let nonce = data.sequence_number_range.as_ref()
            .and_then(|seq| seq.last().copied());

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.datetime.clone().or_else(|| {
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
            }),
            nonce,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true, // Bullish l2Orderbook only sends snapshots
        }
    }

    /// 체결 메시지 파싱
    fn parse_trades(data: &BullishWsTradesData) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.symbol);

        let trades: Vec<Trade> = data.trades.iter().map(|t| {
            let timestamp = t.created_at_timestamp
                .as_ref()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let price = t.price.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();
            let amount = t.quantity.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default();

            Trade {
                id: t.trade_id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: t.created_at_datetime.clone().or_else(|| {
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                }),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: if t.is_taker.unwrap_or(false) {
                    Some(crate::types::TakerOrMaker::Taker)
                } else {
                    Some(crate::types::TakerOrMaker::Maker)
                },
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: vec![],
                info: serde_json::to_value(t).unwrap_or_default(),
            }
        }).collect();

        WsTradeEvent {
            symbol: unified_symbol,
            trades,
        }
    }

    /// WebSocket 메시지 처리
    async fn handle_message(
        message: &str,
        tx: &mpsc::UnboundedSender<WsMessage>,
        _order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
    ) {
        if let Ok(msg) = serde_json::from_str::<BullishWsMessage>(message) {
            match msg.data_type.as_deref() {
                Some("V1TATickerResponse") => {
                    if let Some(data) = msg.data {
                        if let Ok(ticker_data) = serde_json::from_value::<BullishWsTickerData>(data) {
                            let event = Self::parse_ticker(&ticker_data);
                            let _ = tx.send(WsMessage::Ticker(event));
                        }
                    }
                }
                Some("V1TALevel2") => {
                    if let Some(data) = msg.data {
                        if let Ok(ob_data) = serde_json::from_value::<BullishWsOrderBookData>(data) {
                            let event = Self::parse_order_book(&ob_data);
                            let _ = tx.send(WsMessage::OrderBook(event));
                        }
                    }
                }
                Some("V1TAAnonymousTradeUpdate") => {
                    if let Some(data) = msg.data {
                        if let Ok(trades_data) = serde_json::from_value::<BullishWsTradesData>(data) {
                            let event = Self::parse_trades(&trades_data);
                            let _ = tx.send(WsMessage::Trade(event));
                        }
                    }
                }
                Some("V1TAErrorResponse") => {
                    if let Some(data) = msg.data {
                        if let Some(err_msg) = data.get("message").and_then(|v| v.as_str()) {
                            let _ = tx.send(WsMessage::Error(err_msg.to_string()));
                        }
                    }
                }
                _ => {
                    // Handle pong and other messages
                    if let Some(result) = msg.result {
                        if let Some(msg_str) = result.get("message").and_then(|v| v.as_str()) {
                            if msg_str == "Keep alive pong" {
                                // Pong received, connection is alive
                            }
                        }
                    }
                }
            }
        }
    }

    /// 티커용 WebSocket 연결 (직접 연결, 구독 메시지 불필요)
    async fn connect_ticker(
        &self,
        symbol: &str,
        tx: mpsc::UnboundedSender<WsMessage>,
    ) -> CcxtResult<()> {
        use tokio_tungstenite::connect_async;
        use futures_util::{SinkExt, StreamExt};

        let market_id = Self::format_symbol(symbol);
        let url = format!("{}/trading-api/v1/market-data/tick/{}", WS_PUBLIC_URL, market_id);

        let (ws_stream, _) = connect_async(&url).await.map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        let (mut write, mut read) = ws_stream.split();
        let _ = tx.send(WsMessage::Connected);

        let order_books = self.order_books.clone();
        let request_id_atomic = AtomicI64::new(self.request_id.load(Ordering::SeqCst));

        tokio::spawn(async move {
            let mut ping_interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                Self::handle_message(&text, &tx, order_books.clone()).await;
                            }
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                                let _ = write.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await;
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(WsMessage::Error(e.to_string()));
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                    _ = ping_interval.tick() => {
                        // Send keepalive ping
                        let id = request_id_atomic.fetch_add(1, Ordering::SeqCst) + 1;
                        let ping_msg = serde_json::json!({
                            "jsonrpc": "2.0",
                            "type": "command",
                            "method": "keepalivePing",
                            "params": {},
                            "id": id.to_string()
                        });
                        let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(ping_msg.to_string())).await;
                    }
                }
            }
        });

        Ok(())
    }

    /// WebSocket 연결 및 구독 (orderbook, trades)
    async fn connect_and_subscribe(
        &self,
        path: &str,
        topic: &str,
        symbol: &str,
        tx: mpsc::UnboundedSender<WsMessage>,
    ) -> CcxtResult<()> {
        use tokio_tungstenite::connect_async;
        use futures_util::{SinkExt, StreamExt};

        let url = format!("{}{}", WS_PUBLIC_URL, path);
        let market_id = Self::format_symbol(symbol);

        let (ws_stream, _) = connect_async(&url).await.map_err(|e| CcxtError::NetworkError {
            url: url.clone(),
            message: e.to_string(),
        })?;

        let (mut write, mut read) = ws_stream.split();
        let _ = tx.send(WsMessage::Connected);

        // Send subscription message
        let subscribe_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "type": "command",
            "method": "subscribe",
            "params": {
                "topic": topic,
                "symbol": market_id
            },
            "id": self.next_request_id().to_string()
        });

        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                subscribe_msg.to_string(),
            ))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: url.clone(),
                message: e.to_string(),
            })?;

        let order_books = self.order_books.clone();
        let request_id_atomic = AtomicI64::new(self.request_id.load(Ordering::SeqCst));

        tokio::spawn(async move {
            let mut ping_interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                Self::handle_message(&text, &tx, order_books.clone()).await;
                            }
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(data))) => {
                                let _ = write.send(tokio_tungstenite::tungstenite::Message::Pong(data)).await;
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(WsMessage::Error(e.to_string()));
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                    _ = ping_interval.tick() => {
                        // Send keepalive ping
                        let id = request_id_atomic.fetch_add(1, Ordering::SeqCst) + 1;
                        let ping_msg = serde_json::json!({
                            "jsonrpc": "2.0",
                            "type": "command",
                            "method": "keepalivePing",
                            "params": {},
                            "id": id.to_string()
                        });
                        let _ = write.send(tokio_tungstenite::tungstenite::Message::Text(ping_msg.to_string())).await;
                    }
                }
            }
        });

        Ok(())
    }
}

#[async_trait]
impl WsExchange for BullishWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.connect_ticker(symbol, tx).await?;
        Ok(rx)
    }

    async fn watch_tickers(
        &self,
        _symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bullish doesn't support watchTickers
        Err(CcxtError::NotSupported {
            feature: "bullish:watch_tickers".to_string(),
        })
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.connect_and_subscribe(
            "/trading-api/v1/market-data/orderbook",
            "l2Orderbook",
            symbol,
            tx,
        ).await?;
        Ok(rx)
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.connect_and_subscribe(
            "/trading-api/v1/market-data/trades",
            "anonymousTrades",
            symbol,
            tx,
        ).await?;
        Ok(rx)
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Bullish doesn't support watchOHLCV
        Err(CcxtError::NotSupported {
            feature: "bullish:watch_ohlcv".to_string(),
        })
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        *self._connected.write().await = true;
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        *self._connected.write().await = false;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        *self._connected.read().await
    }
}

// WebSocket 메시지 타입 정의
#[derive(Debug, Deserialize)]
struct BullishWsMessage {
    r#type: Option<String>,
    #[serde(rename = "dataType")]
    data_type: Option<String>,
    data: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BullishWsTickerData {
    symbol: String,
    #[serde(rename = "createdAtTimestamp")]
    created_at_timestamp: Option<String>,
    #[serde(rename = "createdAtDatetime")]
    created_at_datetime: Option<String>,
    #[serde(rename = "publishedAtTimestamp")]
    published_at_timestamp: Option<String>,
    high: Option<String>,
    low: Option<String>,
    open: Option<String>,
    close: Option<String>,
    last: Option<String>,
    #[serde(rename = "bestBid")]
    best_bid: Option<String>,
    #[serde(rename = "bestAsk")]
    best_ask: Option<String>,
    #[serde(rename = "bidVolume")]
    bid_volume: Option<String>,
    #[serde(rename = "askVolume")]
    ask_volume: Option<String>,
    #[serde(rename = "baseVolume")]
    base_volume: Option<String>,
    #[serde(rename = "quoteVolume")]
    quote_volume: Option<String>,
    vwap: Option<String>,
    change: Option<String>,
    percentage: Option<String>,
    average: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BullishWsOrderBookData {
    symbol: String,
    timestamp: Option<String>,
    datetime: Option<String>,
    bids: Vec<String>,
    asks: Vec<String>,
    #[serde(rename = "sequenceNumberRange")]
    sequence_number_range: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
struct BullishWsTradesData {
    symbol: String,
    trades: Vec<BullishWsTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BullishWsTrade {
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(rename = "isTaker")]
    is_taker: Option<bool>,
    price: Option<String>,
    quantity: Option<String>,
    side: Option<String>,
    #[serde(rename = "createdAtTimestamp")]
    created_at_timestamp: Option<String>,
    #[serde(rename = "createdAtDatetime")]
    created_at_datetime: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BullishWs::format_symbol("BTC/USDC"), "BTCUSDC");
        assert_eq!(BullishWs::format_symbol("ETH/USDT"), "ETHUSDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BullishWs::to_unified_symbol("BTCUSDC"), "BTC/USDC");
        assert_eq!(BullishWs::to_unified_symbol("ETHUSDT"), "ETH/USDT");
        assert_eq!(BullishWs::to_unified_symbol("ETHUSD"), "ETH/USD");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = BullishWsTickerData {
            symbol: "BTCUSDC".to_string(),
            created_at_timestamp: Some("1749132838951".to_string()),
            created_at_datetime: Some("2025-06-05T14:13:58.951Z".to_string()),
            published_at_timestamp: Some("1749132838955".to_string()),
            high: Some("105966.6577".to_string()),
            low: Some("104246.6662".to_string()),
            open: Some("104522.4238".to_string()),
            close: Some("104323.9374".to_string()),
            last: Some("104323.9374".to_string()),
            best_bid: Some("104324.5000".to_string()),
            best_ask: Some("104324.6000".to_string()),
            bid_volume: Some("0.00020146".to_string()),
            ask_volume: Some("0.00100822".to_string()),
            base_volume: Some("472.83799258".to_string()),
            quote_volume: Some("49662592.6712".to_string()),
            vwap: Some("105030.6996".to_string()),
            change: Some("-198.4864".to_string()),
            percentage: Some("-0.19".to_string()),
            average: Some("104423.1806".to_string()),
            mark_price: Some("104289.6884".to_string()),
        };

        let event = BullishWs::parse_ticker(&ticker_data);
        assert_eq!(event.symbol, "BTC/USDC");
        assert_eq!(event.ticker.last, Some(Decimal::from_str("104323.9374").unwrap()));
        assert_eq!(event.ticker.high, Some(Decimal::from_str("105966.6577").unwrap()));
    }

    #[test]
    fn test_new() {
        let config = ExchangeConfig::default();
        let ws = BullishWs::new(config);
        assert!(ws.subscriptions.try_read().is_ok());
    }
}
