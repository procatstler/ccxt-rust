//! Phemex WebSocket Implementation
//!
//! Phemex 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://phemex.com/ws";

/// Phemex WebSocket 클라이언트
pub struct PhemexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl PhemexWs {
    /// 새 Phemex WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Phemex 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Phemex 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(phemex_symbol: &str) -> String {
        for quote in &["USDT", "USD", "BTC", "ETH"] {
            if let Some(base) = phemex_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        phemex_symbol.to_string()
    }

    /// Timeframe을 Phemex 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> i32 {
        match timeframe {
            Timeframe::Minute1 => 60,
            Timeframe::Minute5 => 300,
            Timeframe::Minute15 => 900,
            Timeframe::Minute30 => 1800,
            Timeframe::Hour1 => 3600,
            Timeframe::Hour4 => 14400,
            Timeframe::Day1 => 86400,
            Timeframe::Week1 => 604800,
            Timeframe::Month1 => 2592000,
            _ => 60,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &PhemexTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let scale = Decimal::new(10_i64.pow(8), 0);

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_ev.map(|v| Decimal::from(v) / scale),
            low: data.low_ev.map(|v| Decimal::from(v) / scale),
            bid: data.bid_ev.map(|v| Decimal::from(v) / scale),
            bid_volume: None,
            ask: data.ask_ev.map(|v| Decimal::from(v) / scale),
            ask_volume: None,
            vwap: None,
            open: data.open_ev.map(|v| Decimal::from(v) / scale),
            close: data.last_ev.map(|v| Decimal::from(v) / scale),
            last: data.last_ev.map(|v| Decimal::from(v) / scale),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_ev.map(|v| Decimal::from(v) / scale),
            quote_volume: data.turnover_ev.map(|v| Decimal::from(v) / scale),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &PhemexOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from(b[0]) / scale,
                    amount: Decimal::from(b[1]) / scale,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: Decimal::from(a[0]) / scale,
                    amount: Decimal::from(a[1]) / scale,
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
            nonce: None,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &PhemexTradeData, symbol: &str) -> WsTradeEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = Decimal::from(data.price_ev.unwrap_or(0)) / scale;
        let amount = Decimal::from(data.qty_ev.unwrap_or(0)) / scale;

        let trades = vec![Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            order: None,
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
        }];

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// 캔들 메시지 파싱
    fn parse_candle(data: &PhemexKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let scale = Decimal::new(10_i64.pow(8), 0);

        let ohlcv = OHLCV {
            timestamp: data.timestamp.unwrap_or(0),
            open: Decimal::from(data.open_ev.unwrap_or(0)) / scale,
            high: Decimal::from(data.high_ev.unwrap_or(0)) / scale,
            low: Decimal::from(data.low_ev.unwrap_or(0)) / scale,
            close: Decimal::from(data.close_ev.unwrap_or(0)) / scale,
            volume: Decimal::from(data.volume_ev.unwrap_or(0)) / scale,
        };

        WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Pong 메시지 처리
        if msg.contains("\"pong\"") {
            return None;
        }

        let response: PhemexWsResponse = serde_json::from_str(msg).ok()?;

        // 에러 처리
        if response.error.is_some() {
            return Some(WsMessage::Error(format!("{:?}", response.error)));
        }

        // 티커
        if let Some(ticker_data) = response.tick {
            if let Some(symbol) = subscribed_symbol {
                let _unified = Self::to_unified_symbol(symbol);
                let mut ticker_data = ticker_data;
                ticker_data.symbol = symbol.to_string();
                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
            }
        }

        // 호가창
        if let Some(book_data) = response.book {
            if let Some(symbol) = subscribed_symbol {
                let unified = Self::to_unified_symbol(symbol);
                return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, &unified)));
            }
        }

        // 체결
        if let Some(trades) = response.trades {
            if let Some(first) = trades.first() {
                if let Some(symbol) = subscribed_symbol {
                    let unified = Self::to_unified_symbol(symbol);
                    return Some(WsMessage::Trade(Self::parse_trade(first, &unified)));
                }
            }
        }

        // 캔들
        if let Some(kline_data) = response.kline {
            if let Some(symbol) = subscribed_symbol {
                let unified = Self::to_unified_symbol(symbol);
                let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                return Some(WsMessage::Ohlcv(Self::parse_candle(&kline_data, &unified, timeframe)));
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        subscribe_msg: serde_json::Value,
        channel: &str,
        symbol: Option<&str>,
        timeframe: Option<Timeframe>
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
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
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref(), timeframe) {
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

impl Default for PhemexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for PhemexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": 1,
            "method": "tick.subscribe",
            "params": [formatted]
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let params: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        let subscribe_msg = serde_json::json!({
            "id": 1,
            "method": "tick.subscribe",
            "params": params
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": 2,
            "method": "orderbook.subscribe",
            "params": [formatted]
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": 3,
            "method": "trade.subscribe",
            "params": [formatted]
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "id": 4,
            "method": "kline.subscribe",
            "params": [formatted, interval]
        });
        client.subscribe_stream(subscribe_msg, "kline", Some(&formatted), Some(timeframe)).await
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

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct PhemexWsResponse {
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    tick: Option<PhemexTickerData>,
    #[serde(default)]
    book: Option<PhemexOrderBookData>,
    #[serde(default)]
    trades: Option<Vec<PhemexTradeData>>,
    #[serde(default)]
    kline: Option<PhemexKlineData>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexTickerData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "lastEp")]
    last_ev: Option<i64>,
    #[serde(default, rename = "openEp")]
    open_ev: Option<i64>,
    #[serde(default, rename = "highEp")]
    high_ev: Option<i64>,
    #[serde(default, rename = "lowEp")]
    low_ev: Option<i64>,
    #[serde(default, rename = "bidEp")]
    bid_ev: Option<i64>,
    #[serde(default, rename = "askEp")]
    ask_ev: Option<i64>,
    #[serde(default, rename = "volumeEv")]
    volume_ev: Option<i64>,
    #[serde(default, rename = "turnoverEv")]
    turnover_ev: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexOrderBookData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<i64>>,
    #[serde(default)]
    asks: Vec<Vec<i64>>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexTradeData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "priceEp")]
    price_ev: Option<i64>,
    #[serde(default, rename = "qtyEv")]
    qty_ev: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexKlineData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "openEp")]
    open_ev: Option<i64>,
    #[serde(default, rename = "highEp")]
    high_ev: Option<i64>,
    #[serde(default, rename = "lowEp")]
    low_ev: Option<i64>,
    #[serde(default, rename = "closeEp")]
    close_ev: Option<i64>,
    #[serde(default, rename = "volumeEv")]
    volume_ev: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(PhemexWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(PhemexWs::format_symbol("ETH/USD"), "ETHUSD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(PhemexWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(PhemexWs::to_unified_symbol("ETHUSD"), "ETH/USD");
    }
}
