//! BitMart WebSocket Implementation
//!
//! BitMart 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
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

const WS_PUBLIC_URL: &str = "wss://ws-manager-compress.bitmart.com/api?protocol=1.1";

/// BitMart WebSocket 클라이언트
pub struct BitmartWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BitmartWs {
    /// 새 BitMart WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 BitMart 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// BitMart 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(bitmart_symbol: &str) -> String {
        bitmart_symbol.replace("_", "/")
    }

    /// Timeframe을 BitMart 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour4 => "4H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BitmartTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ms_t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.best_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open_24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.price_change_24h.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.base_volume_24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume_24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BitmartOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let timestamp = data.ms_t.unwrap_or_else(|| Utc::now().timestamp_millis());
        let unified_symbol = Self::to_unified_symbol(symbol);

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

    /// 체결 메시지 파싱
    fn parse_trade(data: &BitmartTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.s_t.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
        let unified_symbol = Self::to_unified_symbol(symbol);
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.size.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let trades = vec![Trade {
            id: String::new(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            trades,
        }
    }

    /// 캔들 메시지 파싱
    fn parse_candle(data: &BitmartKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let ohlcv = OHLCV {
            timestamp: data.candle.first()
                .and_then(|t| t.parse::<i64>().ok())
                .unwrap_or(0) * 1000,
            open: data.candle.get(1)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            high: data.candle.get(2)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            low: data.candle.get(3)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            close: data.candle.get(4)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            volume: data.candle.get(5)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Pong 메시지 처리
        if msg.contains("\"pong\"") || msg.contains("pong") {
            return None;
        }

        let response: BitmartWsResponse = serde_json::from_str(msg).ok()?;
        let table = response.table.as_deref()?;
        let data = response.data?;
        let symbol = subscribed_symbol.unwrap_or("");

        match table {
            s if s.contains("ticker") => {
                if let Some(first) = data.first() {
                    if let Ok(ticker_data) = serde_json::from_value::<BitmartTickerData>(first.clone()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }
            }
            s if s.contains("depth") => {
                if let Some(first) = data.first() {
                    if let Ok(book_data) = serde_json::from_value::<BitmartOrderBookData>(first.clone()) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, symbol)));
                    }
                }
            }
            s if s.contains("trade") => {
                if let Some(first) = data.first() {
                    if let Ok(trade_data) = serde_json::from_value::<BitmartTradeData>(first.clone()) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                    }
                }
            }
            s if s.contains("kline") => {
                if let Some(first) = data.first() {
                    if let Ok(kline_data) = serde_json::from_value::<BitmartKlineData>(first.clone()) {
                        let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                        return Some(WsMessage::Ohlcv(Self::parse_candle(&kline_data, symbol, timeframe)));
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
        timeframe: Option<Timeframe>
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 15,
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

impl Default for BitmartWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitmartWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/ticker:{}", formatted)]
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args: Vec<String> = symbols.iter()
            .map(|s| format!("spot/ticker:{}", Self::format_symbol(s)))
            .collect();
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(20);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/depth{}:{}", depth, formatted)]
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/trade:{}", formatted)]
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": [format!("spot/kline{}:{}", interval, formatted)]
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
struct BitmartWsResponse {
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartTickerData {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    ms_t: Option<i64>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    open_24h: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(default)]
    base_volume_24h: Option<String>,
    #[serde(default)]
    quote_volume_24h: Option<String>,
    #[serde(default)]
    price_change_24h: Option<String>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartOrderBookData {
    #[serde(default)]
    ms_t: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartTradeData {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    s_t: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartKlineData {
    #[serde(default)]
    candle: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitmartWs::format_symbol("BTC/USDT"), "BTC_USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitmartWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
    }
}
