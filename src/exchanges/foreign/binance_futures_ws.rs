//! Binance Futures (USDⓈ-M) WebSocket Implementation
//!
//! Binance USDⓈ-M Futures 실시간 데이터 스트리밍

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_BASE_URL: &str = "wss://fstream.binance.com/ws";
const WS_COMBINED_URL: &str = "wss://fstream.binance.com/stream";

/// Binance Futures WebSocket 클라이언트
pub struct BinanceFuturesWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BinanceFuturesWs {
    /// 새 Binance Futures WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Binance 형식으로 변환 (BTC/USDT:USDT -> btcusdt)
    fn format_symbol(symbol: &str) -> String {
        // Remove settle currency (BTC/USDT:USDT -> BTC/USDT)
        let base_symbol = symbol.split(':').next().unwrap_or(symbol);
        base_symbol.replace("/", "").to_lowercase()
    }

    /// Timeframe을 Binance kline interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Second1 => "1s",
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour3 => "3h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
        }
    }

    /// Binance 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT:USDT)
    fn to_unified_symbol(binance_symbol: &str) -> String {
        // Quote currencies for futures
        let quotes = ["USDT", "BUSD", "USDC"];

        for quote in quotes {
            if let Some(base) = binance_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}:{quote}");
            }
        }

        binance_symbol.to_string()
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BinanceFuturesTicker) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h.as_ref().and_then(|v| v.parse().ok()),
            low: data.l.as_ref().and_then(|v| v.parse().ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: data.w.as_ref().and_then(|v| v.parse().ok()),
            open: data.o.as_ref().and_then(|v| v.parse().ok()),
            close: data.c.as_ref().and_then(|v| v.parse().ok()),
            last: data.c.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.p.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.P.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.q.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Mark Price 티커 파싱
    fn parse_mark_price_ticker(data: &BinanceMarkPriceUpdate) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: None,
            bid_volume: None,
            ask: None,
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
            index_price: data.i.as_ref().and_then(|v| v.parse().ok()),
            mark_price: data.p.as_ref().and_then(|v| v.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BinanceFuturesDepthUpdate, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.b.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0].parse().ok()?,
                    amount: b[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.a.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0].parse().ok()?,
                    amount: a[1].parse().ok()?,
                })
            } else {
                None
            }
        }).collect();

        let timestamp = data.E.unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.u,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BinanceFuturesTradeMsg) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.T.unwrap_or_else(|| Utc::now().timestamp_millis());

        let trade = Trade {
            id: data.t.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: if data.m { Some("sell".into()) } else { Some("buy".into()) },
            taker_or_maker: None,
            price: data.p.parse().unwrap_or_default(),
            amount: data.q.parse().unwrap_or_default(),
            cost: Some(
                data.p.parse::<Decimal>().unwrap_or_default()
                    * data.q.parse::<Decimal>().unwrap_or_default(),
            ),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// Kline 메시지 파싱
    fn parse_kline(data: &BinanceFuturesKlineMsg, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.s);
        let k = &data.k;

        let ohlcv = OHLCV {
            timestamp: k.t,
            open: k.o.parse().unwrap_or_default(),
            high: k.h.parse().unwrap_or_default(),
            low: k.l.parse().unwrap_or_default(),
            close: k.c.parse().unwrap_or_default(),
            volume: k.v.parse().unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Error check
        if let Ok(err) = serde_json::from_str::<BinanceFuturesError>(msg) {
            if err.code.is_some() {
                return Some(WsMessage::Error(err.msg.unwrap_or_default()));
            }
        }

        // 24hr ticker
        if msg.contains("\"e\":\"24hrTicker\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesTicker>(msg) {
                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
            }
        }

        // miniTicker
        if msg.contains("\"e\":\"24hrMiniTicker\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesTicker>(msg) {
                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
            }
        }

        // Mark price
        if msg.contains("\"e\":\"markPriceUpdate\"") {
            if let Ok(data) = serde_json::from_str::<BinanceMarkPriceUpdate>(msg) {
                return Some(WsMessage::Ticker(Self::parse_mark_price_ticker(&data)));
            }
        }

        // Depth update
        if msg.contains("\"e\":\"depthUpdate\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesDepthUpdate>(msg) {
                let symbol = Self::to_unified_symbol(&data.s);
                return Some(WsMessage::OrderBook(Self::parse_order_book(&data, &symbol)));
            }
        }

        // Aggregate trade
        if msg.contains("\"e\":\"aggTrade\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesTradeMsg>(msg) {
                return Some(WsMessage::Trade(Self::parse_trade(&data)));
            }
        }

        // Kline
        if msg.contains("\"e\":\"kline\"") {
            if let Ok(data) = serde_json::from_str::<BinanceFuturesKlineMsg>(msg) {
                let timeframe = match data.k.i.as_str() {
                    "1s" => Timeframe::Second1,
                    "1m" => Timeframe::Minute1,
                    "3m" => Timeframe::Minute3,
                    "5m" => Timeframe::Minute5,
                    "15m" => Timeframe::Minute15,
                    "30m" => Timeframe::Minute30,
                    "1h" => Timeframe::Hour1,
                    "2h" => Timeframe::Hour2,
                    "4h" => Timeframe::Hour4,
                    "6h" => Timeframe::Hour6,
                    "8h" => Timeframe::Hour8,
                    "12h" => Timeframe::Hour12,
                    "1d" => Timeframe::Day1,
                    "3d" => Timeframe::Day3,
                    "1w" => Timeframe::Week1,
                    "1M" => Timeframe::Month1,
                    _ => Timeframe::Minute1,
                };
                return Some(WsMessage::Ohlcv(Self::parse_kline(&data, timeframe)));
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, stream: &str, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = format!("{WS_BASE_URL}/{stream}");

        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, stream.to_string());
        }

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

    /// 복수 스트림 구독
    async fn subscribe_combined_streams(&mut self, streams: Vec<String>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let combined = streams.join("/");
        let url = format!("{WS_COMBINED_URL}?streams={combined}");

        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

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
                        // Combined stream format: {"stream":"btcusdt@aggTrade","data":{...}}
                        if let Ok(combined) = serde_json::from_str::<BinanceFuturesCombinedStream>(&msg) {
                            if let Some(ws_msg) = Self::process_message(&serde_json::to_string(&combined.data).unwrap_or_default()) {
                                let _ = tx.send(ws_msg);
                            }
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

impl Default for BinanceFuturesWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BinanceFuturesWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let stream = format!("{}@ticker", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let streams: Vec<String> = symbols
            .iter()
            .map(|s| format!("{}@ticker", Self::format_symbol(s)))
            .collect();
        client.subscribe_combined_streams(streams).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let depth = limit.unwrap_or(10).min(20);
        // Futures uses different depth stream format
        let stream = format!("{}@depth{}@100ms", Self::format_symbol(symbol), depth);
        client.subscribe_stream(&stream, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        // Futures uses aggTrade
        let stream = format!("{}@aggTrade", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let stream = format!("{}@kline_{}", Self::format_symbol(symbol), interval);
        client.subscribe_stream(&stream, "ohlcv", Some(symbol)).await
    }

    async fn watch_mark_price(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        // @markPrice@1s provides mark price updates every second
        let stream = format!("{}@markPrice@1s", Self::format_symbol(symbol));
        client.subscribe_stream(&stream, "markPrice", Some(symbol)).await
    }

    async fn watch_mark_prices(&self, symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        if let Some(syms) = symbols {
            let streams: Vec<String> = syms
                .iter()
                .map(|s| format!("{}@markPrice@1s", Self::format_symbol(s)))
                .collect();
            client.subscribe_combined_streams(streams).await
        } else {
            // Subscribe to all mark prices using !markPrice@arr stream
            client.subscribe_stream("!markPrice@arr@1s", "markPrice", None).await
        }
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

// === Binance Futures WebSocket Types ===

#[derive(Debug, Deserialize)]
struct BinanceFuturesError {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BinanceFuturesCombinedStream {
    #[allow(dead_code)]
    stream: String,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesTicker {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    #[serde(default)]
    p: Option<String>,  // Price change
    #[serde(default)]
    P: Option<String>,  // Price change percent
    #[serde(default)]
    w: Option<String>,  // Weighted average price
    #[serde(default)]
    c: Option<String>,  // Last price
    #[serde(default)]
    Q: Option<String>,  // Last quantity
    #[serde(default)]
    o: Option<String>,  // Open price
    #[serde(default)]
    h: Option<String>,  // High price
    #[serde(default)]
    l: Option<String>,  // Low price
    #[serde(default)]
    v: Option<String>,  // Base volume
    #[serde(default)]
    q: Option<String>,  // Quote volume
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceMarkPriceUpdate {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    #[serde(default)]
    p: Option<String>,  // Mark price
    #[serde(default)]
    i: Option<String>,  // Index price
    #[serde(default)]
    P: Option<String>,  // Estimated settle price
    #[serde(default)]
    r: Option<String>,  // Funding rate
    #[serde(default)]
    T: Option<i64>,     // Next funding time
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesDepthUpdate {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    #[serde(default)]
    T: Option<i64>,
    s: String,
    #[serde(default)]
    U: Option<i64>,
    #[serde(default)]
    u: Option<i64>,
    #[serde(default)]
    pu: Option<i64>,  // Previous final update ID
    #[serde(default)]
    b: Vec<Vec<String>>,
    #[serde(default)]
    a: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesTradeMsg {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    #[serde(default, rename = "a")]
    t: i64,  // Aggregate trade ID (use 'a' field)
    p: String,
    q: String,
    #[serde(default)]
    f: Option<i64>,  // First trade ID
    #[serde(default)]
    l: Option<i64>,  // Last trade ID
    #[serde(default)]
    T: Option<i64>,
    m: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesKlineMsg {
    e: String,
    #[serde(default)]
    E: Option<i64>,
    s: String,
    k: BinanceFuturesKline,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BinanceFuturesKline {
    t: i64,
    #[serde(default)]
    T: Option<i64>,
    s: String,
    i: String,
    #[serde(default)]
    f: Option<i64>,  // First trade ID
    #[serde(default)]
    L: Option<i64>,  // Last trade ID
    o: String,
    c: String,
    h: String,
    l: String,
    v: String,
    #[serde(default)]
    n: Option<i64>,  // Number of trades
    #[serde(default)]
    x: Option<bool>,
    #[serde(default)]
    q: Option<String>,  // Quote volume
    #[serde(default)]
    V: Option<String>,  // Taker buy base volume
    #[serde(default)]
    Q: Option<String>,  // Taker buy quote volume
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BinanceFuturesWs::format_symbol("BTC/USDT:USDT"), "btcusdt");
        assert_eq!(BinanceFuturesWs::format_symbol("ETH/USDT:USDT"), "ethusdt");
        assert_eq!(BinanceFuturesWs::format_symbol("BTC/USDT"), "btcusdt");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BinanceFuturesWs::to_unified_symbol("BTCUSDT"), "BTC/USDT:USDT");
        assert_eq!(BinanceFuturesWs::to_unified_symbol("ETHUSDT"), "ETH/USDT:USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BinanceFuturesWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BinanceFuturesWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(BinanceFuturesWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_ticker() {
        let json = r#"{"e":"24hrTicker","E":1234567890000,"s":"BTCUSDT","p":"-50.00","P":"-0.1","w":"50000.00","c":"50000.00","o":"49000.00","h":"51000.00","l":"48000.00","v":"1000.00","q":"50000000.00"}"#;
        let data: BinanceFuturesTicker = serde_json::from_str(json).unwrap();
        let event = BinanceFuturesWs::parse_ticker(&data);

        assert_eq!(event.symbol, "BTC/USDT:USDT");
        assert_eq!(event.ticker.close, Some(Decimal::new(5000000, 2)));
    }
}
