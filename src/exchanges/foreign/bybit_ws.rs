//! Bybit WebSocket Implementation
//!
//! Bybit 실시간 데이터 스트리밍

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://stream.bybit.com/v5/public/spot";
const WS_LINEAR_URL: &str = "wss://stream.bybit.com/v5/public/linear";

/// Bybit WebSocket 클라이언트
pub struct BybitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BybitWs {
    /// 새 Bybit WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Bybit 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_uppercase()
    }

    /// Timeframe을 Bybit interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute3 => "3",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour2 => "120",
            Timeframe::Hour4 => "240",
            Timeframe::Hour6 => "360",
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "D",
            Timeframe::Week1 => "W",
            Timeframe::Month1 => "M",
            _ => "1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BybitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_price_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid1_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid1_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask1_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask1_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: None,
            close: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: data.prev_price_24h.as_ref().and_then(|v| v.parse().ok()),
            change: data.price_24h_pcnt.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.price_24h_pcnt.as_ref().and_then(|v| v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))),
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.turnover_24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BybitOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BybitTradeData, symbol: &str) -> WsTradeEvent {
        let trades: Vec<Trade> = vec![{
            let timestamp = data.T.unwrap_or_else(|| Utc::now().timestamp_millis());
            let price: Decimal = data.p.parse().unwrap_or_default();
            let amount: Decimal = data.v.parse().unwrap_or_default();

            Trade {
                id: data.i.clone(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(data.S.to_lowercase()),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(data).unwrap_or_default(),
            }
        }];

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades,
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_kline(data: &BybitKlineData, timeframe: Timeframe) -> WsOhlcvEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.start;

        let ohlcv = OHLCV::new(
            timestamp,
            data.open.parse().unwrap_or_default(),
            data.high.parse().unwrap_or_default(),
            data.low.parse().unwrap_or_default(),
            data.close.parse().unwrap_or_default(),
            data.volume.parse().unwrap_or_default(),
        );

        WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }
    }

    /// Bybit 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(bybit_symbol: &str) -> String {
        let quotes = ["USDT", "USDC", "BTC", "ETH", "EUR", "DAI"];

        for quote in quotes {
            if let Some(base) = bybit_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        bybit_symbol.to_string()
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Bybit 메시지 파싱
        if let Ok(response) = serde_json::from_str::<BybitWsResponse>(msg) {
            // 에러 체크
            if response.ret_code.is_some() && response.ret_code != Some(0) {
                return Some(WsMessage::Error(response.ret_msg.unwrap_or_default()));
            }

            // 구독 확인
            if response.op == Some("subscribe".to_string()) {
                return Some(WsMessage::Subscribed {
                    channel: "".to_string(),
                    symbol: None,
                });
            }

            // 데이터 처리
            if let Some(topic) = &response.topic {
                if let Some(data) = &response.data {
                    // Ticker
                    if topic.starts_with("tickers.") {
                        if let Ok(ticker_data) = serde_json::from_value::<BybitTickerData>(data.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // OrderBook
                    if topic.starts_with("orderbook.") {
                        if let Ok(ob_data) = serde_json::from_value::<BybitOrderBookData>(data.clone()) {
                            let symbol = Self::to_unified_symbol(&ob_data.s);
                            let is_snapshot = response.response_type == Some("snapshot".to_string());
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                        }
                    }

                    // Trade
                    if topic.starts_with("publicTrade.") {
                        if let Ok(trades) = serde_json::from_value::<Vec<BybitTradeData>>(data.clone()) {
                            if let Some(trade_data) = trades.first() {
                                let symbol = Self::to_unified_symbol(&trade_data.s);
                                return Some(WsMessage::Trade(Self::parse_trade(trade_data, &symbol)));
                            }
                        }
                    }

                    // Kline
                    if topic.starts_with("kline.") {
                        if let Ok(kline_list) = serde_json::from_value::<Vec<BybitKlineData>>(data.clone()) {
                            if let Some(kline_data) = kline_list.first() {
                                // Extract interval from topic (e.g., "kline.1.BTCUSDT")
                                let parts: Vec<&str> = topic.split('.').collect();
                                let timeframe = if parts.len() >= 2 {
                                    match parts[1] {
                                        "1" => Timeframe::Minute1,
                                        "3" => Timeframe::Minute3,
                                        "5" => Timeframe::Minute5,
                                        "15" => Timeframe::Minute15,
                                        "30" => Timeframe::Minute30,
                                        "60" => Timeframe::Hour1,
                                        "120" => Timeframe::Hour2,
                                        "240" => Timeframe::Hour4,
                                        "360" => Timeframe::Hour6,
                                        "720" => Timeframe::Hour12,
                                        "D" => Timeframe::Day1,
                                        "W" => Timeframe::Week1,
                                        "M" => Timeframe::Month1,
                                        _ => Timeframe::Minute1,
                                    }
                                } else {
                                    Timeframe::Minute1
                                };
                                return Some(WsMessage::Ohlcv(Self::parse_kline(kline_data, timeframe)));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, topics: Vec<String>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
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
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": topics
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, topics.join(","));
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

impl Default for BybitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BybitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topic = format!("tickers.{}", Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topics: Vec<String> = symbols
            .iter()
            .map(|s| format!("tickers.{}", Self::format_symbol(s)))
            .collect();
        client.subscribe_stream(topics, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let depth = match limit.unwrap_or(50) {
            1 => 1,
            25 => 25,
            50 => 50,
            100 => 100,
            200 => 200,
            _ => 50,
        };
        let topic = format!("orderbook.{}.{}", depth, Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topic = format!("publicTrade.{}", Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let topic = format!("kline.{}.{}", interval, Self::format_symbol(symbol));
        client.subscribe_stream(vec![topic], "ohlcv", Some(symbol)).await
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

// === Bybit WebSocket Message Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BybitWsResponse {
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default, rename = "type")]
    response_type: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default, rename = "retCode")]
    ret_code: Option<i32>,
    #[serde(default, rename = "retMsg")]
    ret_msg: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BybitTickerData {
    symbol: String,
    #[serde(default, rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(default, rename = "highPrice24h")]
    high_price_24h: Option<String>,
    #[serde(default, rename = "lowPrice24h")]
    low_price_24h: Option<String>,
    #[serde(default, rename = "prevPrice24h")]
    prev_price_24h: Option<String>,
    #[serde(default, rename = "volume24h")]
    volume_24h: Option<String>,
    #[serde(default, rename = "turnover24h")]
    turnover_24h: Option<String>,
    #[serde(default, rename = "price24hPcnt")]
    price_24h_pcnt: Option<String>,
    #[serde(default, rename = "bid1Price")]
    bid1_price: Option<String>,
    #[serde(default, rename = "bid1Size")]
    bid1_size: Option<String>,
    #[serde(default, rename = "ask1Price")]
    ask1_price: Option<String>,
    #[serde(default, rename = "ask1Size")]
    ask1_size: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BybitOrderBookData {
    s: String, // Symbol
    #[serde(default)]
    b: Vec<Vec<String>>, // Bids [price, size]
    #[serde(default)]
    a: Vec<Vec<String>>, // Asks [price, size]
    #[serde(default)]
    u: Option<i64>, // Update ID
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct BybitTradeData {
    s: String,  // Symbol
    i: String,  // Trade ID
    p: String,  // Price
    v: String,  // Size/Volume
    S: String,  // Side: Buy/Sell
    #[serde(default)]
    T: Option<i64>, // Trade time
    #[serde(default)]
    BT: Option<bool>, // Block trade
}

#[derive(Debug, Deserialize, Serialize)]
struct BybitKlineData {
    symbol: String,
    start: i64,
    end: i64,
    interval: String,
    open: String,
    close: String,
    high: String,
    low: String,
    volume: String,
    turnover: String,
    #[serde(default)]
    confirm: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BybitWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(BybitWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BybitWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(BybitWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BybitWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(BybitWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(BybitWs::format_interval(Timeframe::Day1), "D");
    }
}
