//! OneTrading WebSocket Implementation
//!
//! OneTrading (formerly Bitpanda Pro) 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::ExchangeConfig;
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, OHLCV,
    WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent, WsTickerEvent,
};

const WS_BASE_URL: &str = "wss://streams.onetrading.com/";

/// OneTrading WebSocket 클라이언트
pub struct OnetradingWs {
    config: ExchangeConfig,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
    _connected: Arc<RwLock<bool>>,
}

impl OnetradingWs {
    /// 새 OneTrading WebSocket 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            order_books: Arc::new(RwLock::new(HashMap::new())),
            _connected: Arc::new(RwLock::new(false)),
        }
    }

    /// 심볼을 OneTrading WebSocket 형식으로 변환 (BTC/EUR -> BTC_EUR)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace('/', "_")
    }

    /// OneTrading 심볼을 통합 심볼로 변환 (BTC_EUR -> BTC/EUR)
    fn to_unified_symbol(onetrading_symbol: &str) -> String {
        onetrading_symbol.replace('_', "/")
    }

    /// Timeframe을 OneTrading 포맷으로 변환
    fn format_granularity(timeframe: Timeframe) -> serde_json::Value {
        let (unit, period) = match timeframe {
            Timeframe::Minute1 => ("MINUTES", 1),
            Timeframe::Minute5 => ("MINUTES", 5),
            Timeframe::Minute15 => ("MINUTES", 15),
            Timeframe::Minute30 => ("MINUTES", 30),
            Timeframe::Hour1 => ("HOURS", 1),
            Timeframe::Hour4 => ("HOURS", 4),
            Timeframe::Day1 => ("DAYS", 1),
            Timeframe::Week1 => ("WEEKS", 1),
            Timeframe::Month1 => ("MONTHS", 1),
            _ => ("MINUTES", 1),
        };
        serde_json::json!({
            "unit": unit,
            "period": period
        })
    }

    /// Timeframe 문자열 반환
    fn timeframe_to_string(timeframe: Timeframe) -> String {
        match timeframe {
            Timeframe::Minute1 => "1m".to_string(),
            Timeframe::Minute5 => "5m".to_string(),
            Timeframe::Minute15 => "15m".to_string(),
            Timeframe::Minute30 => "30m".to_string(),
            Timeframe::Hour1 => "1h".to_string(),
            Timeframe::Hour4 => "4h".to_string(),
            Timeframe::Day1 => "1d".to_string(),
            Timeframe::Week1 => "1w".to_string(),
            Timeframe::Month1 => "1M".to_string(),
            _ => "1m".to_string(),
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &OnetradingWsTicker) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument);
        let timestamp = Utc::now().timestamp_millis();

        let last_price = data.last_price.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let high = data.high.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let low = data.low.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let change = data.price_change.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let percentage = data.price_change_percentage.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let volume = data.volume.as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high,
            low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: last_price,
            last: last_price,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume: None,
            quote_volume: volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book_snapshot(data: &OnetradingWsOrderBookSnapshot) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_code);
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let parse_entries = |entries: &[Vec<String>]| -> Vec<OrderBookEntry> {
            entries
                .iter()
                .filter_map(|e| {
                    if e.len() >= 2 {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&e[0]).ok()?,
                            amount: Decimal::from_str(&e[1]).ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// 호가창 업데이트 파싱
    fn parse_order_book_update(
        data: &OnetradingWsOrderBookUpdate,
        current_book: &OrderBook,
    ) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_code);
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let mut bids = current_book.bids.clone();
        let mut asks = current_book.asks.clone();

        // Apply changes
        for change in &data.changes {
            if change.len() >= 3 {
                let side = &change[0];
                let price = Decimal::from_str(&change[1]).ok();
                let amount = Decimal::from_str(&change[2]).ok();

                if let (Some(price), Some(amount)) = (price, amount) {
                    let entries = if side == "BUY" { &mut bids } else { &mut asks };

                    if amount.is_zero() {
                        // Remove price level
                        entries.retain(|e| e.price != price);
                    } else {
                        // Update or add price level
                        if let Some(entry) = entries.iter_mut().find(|e| e.price == price) {
                            entry.amount = amount;
                        } else {
                            entries.push(OrderBookEntry { price, amount });
                        }
                    }
                }
            }
        }

        // Sort bids (descending) and asks (ascending)
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            order_book,
            is_snapshot: false,
        }
    }

    /// OHLCV 캔들 메시지 파싱
    fn parse_ohlcv(data: &OnetradingWsOhlcv, timeframe: Timeframe) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_code);
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: data.open.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            high: data.high.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            low: data.low.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            close: data.close.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            volume: data.volume.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        }
    }

    /// WebSocket 메시지 처리
    async fn handle_message(
        message: &str,
        tx: &mpsc::UnboundedSender<WsMessage>,
        order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
        timeframe: Option<Timeframe>,
    ) {
        if let Ok(msg) = serde_json::from_str::<OnetradingWsMessage>(message) {
            match msg.r#type.as_deref() {
                Some("MARKET_TICKER_UPDATES") => {
                    if let Some(ticker_updates) = msg.ticker_updates {
                        for ticker_data in ticker_updates {
                            let event = Self::parse_ticker(&ticker_data);
                            let _ = tx.send(WsMessage::Ticker(event));
                        }
                    }
                }
                Some("ORDER_BOOK_SNAPSHOT") => {
                    if let Ok(snapshot) = serde_json::from_str::<OnetradingWsOrderBookSnapshot>(message) {
                        let event = Self::parse_order_book_snapshot(&snapshot);
                        // Store the order book for updates
                        let symbol = event.symbol.clone();
                        {
                            let mut books = order_books.write().await;
                            books.insert(symbol, event.order_book.clone());
                        }
                        let _ = tx.send(WsMessage::OrderBook(event));
                    }
                }
                Some("ORDER_BOOK_UPDATE") => {
                    if let Ok(update) = serde_json::from_str::<OnetradingWsOrderBookUpdate>(message) {
                        let unified_symbol = Self::to_unified_symbol(&update.instrument_code);
                        let books = order_books.read().await;
                        if let Some(current_book) = books.get(&unified_symbol) {
                            let event = Self::parse_order_book_update(&update, current_book);
                            drop(books);
                            {
                                let mut books = order_books.write().await;
                                books.insert(unified_symbol, event.order_book.clone());
                            }
                            let _ = tx.send(WsMessage::OrderBook(event));
                        }
                    }
                }
                Some("CANDLESTICK_SNAPSHOT") | Some("CANDLESTICK") => {
                    if let Ok(ohlcv_data) = serde_json::from_str::<OnetradingWsOhlcv>(message) {
                        let tf = timeframe.unwrap_or(Timeframe::Minute1);
                        let event = Self::parse_ohlcv(&ohlcv_data, tf);
                        let _ = tx.send(WsMessage::Ohlcv(event));
                    }
                }
                Some("SUBSCRIPTIONS") | Some("SUBSCRIPTION_UPDATED") | Some("HEARTBEAT") => {
                    // Subscription confirmations and heartbeats - ignore
                }
                Some("ERROR") => {
                    if let Some(error) = msg.error {
                        let _ = tx.send(WsMessage::Error(error));
                    }
                }
                _ => {}
            }
        }
    }

    /// WebSocket 연결 및 구독
    async fn connect_and_subscribe(
        &self,
        channel: &str,
        symbols: Vec<String>,
        tx: mpsc::UnboundedSender<WsMessage>,
        timeframe: Option<Timeframe>,
    ) -> CcxtResult<()> {
        use tokio_tungstenite::connect_async;
        use futures_util::{SinkExt, StreamExt};

        let url = WS_BASE_URL.to_string();

        let (ws_stream, _) = connect_async(url).await.map_err(|e| CcxtError::NetworkError {
            url: WS_BASE_URL.to_string(),
            message: e.to_string(),
        })?;

        let (mut write, mut read) = ws_stream.split();
        let _ = tx.send(WsMessage::Connected);

        // Build subscription message
        let market_ids: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();

        let subscribe_msg = if channel == "CANDLESTICKS" {
            let tf = timeframe.unwrap_or(Timeframe::Minute1);
            let properties: Vec<serde_json::Value> = market_ids.iter()
                .map(|id| {
                    serde_json::json!({
                        "instrument_code": id,
                        "time_granularity": Self::format_granularity(tf)
                    })
                })
                .collect();

            serde_json::json!({
                "type": "SUBSCRIBE",
                "channels": [{
                    "name": channel,
                    "properties": properties
                }]
            })
        } else if channel == "ORDER_BOOK" {
            serde_json::json!({
                "type": "SUBSCRIBE",
                "channels": [{
                    "name": channel,
                    "instrument_codes": market_ids,
                    "depth": 20
                }]
            })
        } else {
            serde_json::json!({
                "type": "SUBSCRIBE",
                "channels": [{
                    "name": channel,
                    "instrument_codes": market_ids,
                    "price_points_mode": "INLINE"
                }]
            })
        };

        write
            .send(tokio_tungstenite::tungstenite::Message::Text(
                subscribe_msg.to_string(),
            ))
            .await
            .map_err(|e| CcxtError::NetworkError {
                url: WS_BASE_URL.to_string(),
                message: e.to_string(),
            })?;

        let order_books = self.order_books.clone();

        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                        Self::handle_message(&text, &tx, order_books.clone(), timeframe).await;
                    }
                    Ok(tokio_tungstenite::tungstenite::Message::Ping(data)) => {
                        let _ = write
                            .send(tokio_tungstenite::tungstenite::Message::Pong(data))
                            .await;
                    }
                    Err(e) => {
                        let _ = tx.send(WsMessage::Error(e.to_string()));
                        break;
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }
}

#[async_trait]
impl WsExchange for OnetradingWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.connect_and_subscribe("MARKET_TICKER", vec![symbol.to_string()], tx, None)
            .await?;
        Ok(rx)
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let symbols_vec: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
        self.connect_and_subscribe("MARKET_TICKER", symbols_vec, tx, None)
            .await?;
        Ok(rx)
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.connect_and_subscribe("ORDER_BOOK", vec![symbol.to_string()], tx, None)
            .await?;
        Ok(rx)
    }

    async fn watch_trades(&self, _symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // OneTrading doesn't have a public trades channel
        // Return an error indicating this feature is not available
        Err(CcxtError::NotSupported {
            feature: "onetrading:watch_trades".to_string(),
        })
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.connect_and_subscribe("CANDLESTICKS", vec![symbol.to_string()], tx, Some(timeframe))
            .await?;
        Ok(rx)
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
struct OnetradingWsMessage {
    r#type: Option<String>,
    channel_name: Option<String>,
    time: Option<String>,
    ticker_updates: Option<Vec<OnetradingWsTicker>>,
    error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OnetradingWsTicker {
    instrument: String,
    last_price: Option<String>,
    price_change: Option<String>,
    price_change_percentage: Option<String>,
    high: Option<String>,
    low: Option<String>,
    volume: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingWsOrderBookSnapshot {
    instrument_code: String,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    r#type: Option<String>,
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingWsOrderBookUpdate {
    instrument_code: String,
    changes: Vec<Vec<String>>,
    r#type: Option<String>,
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingWsOhlcv {
    instrument_code: String,
    granularity: Option<OnetradingGranularity>,
    open: Option<String>,
    high: Option<String>,
    low: Option<String>,
    close: Option<String>,
    volume: Option<String>,
    time: Option<String>,
    r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnetradingGranularity {
    unit: String,
    period: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(OnetradingWs::format_symbol("BTC/EUR"), "BTC_EUR");
        assert_eq!(OnetradingWs::format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(OnetradingWs::to_unified_symbol("BTC_EUR"), "BTC/EUR");
        assert_eq!(OnetradingWs::to_unified_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_granularity() {
        let granularity = OnetradingWs::format_granularity(Timeframe::Minute1);
        assert_eq!(granularity["unit"], "MINUTES");
        assert_eq!(granularity["period"], 1);

        let granularity = OnetradingWs::format_granularity(Timeframe::Hour4);
        assert_eq!(granularity["unit"], "HOURS");
        assert_eq!(granularity["period"], 4);

        let granularity = OnetradingWs::format_granularity(Timeframe::Day1);
        assert_eq!(granularity["unit"], "DAYS");
        assert_eq!(granularity["period"], 1);
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = OnetradingWsTicker {
            instrument: "BTC_EUR".to_string(),
            last_price: Some("45000.50".to_string()),
            price_change: Some("1000.00".to_string()),
            price_change_percentage: Some("2.27".to_string()),
            high: Some("46000.00".to_string()),
            low: Some("44000.00".to_string()),
            volume: Some("100.5".to_string()),
        };

        let event = OnetradingWs::parse_ticker(&ticker_data);
        assert_eq!(event.symbol, "BTC/EUR");
        assert_eq!(event.ticker.last, Some(Decimal::from_str("45000.50").unwrap()));
        assert_eq!(event.ticker.high, Some(Decimal::from_str("46000.00").unwrap()));
        assert_eq!(event.ticker.low, Some(Decimal::from_str("44000.00").unwrap()));
    }

    #[test]
    fn test_new() {
        let config = ExchangeConfig::default();
        let ws = OnetradingWs::new(config);
        assert!(ws.subscriptions.try_read().is_ok());
    }
}
