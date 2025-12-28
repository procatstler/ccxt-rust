//! BingX WebSocket Implementation
//!
//! BingX 실시간 데이터 스트리밍

#![allow(dead_code)]

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

const WS_PUBLIC_URL: &str = "wss://open-api-ws.bingx.com/market";

/// BingX WebSocket 클라이언트
pub struct BingxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BingxWs {
    /// 새 BingX WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 BingX 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// BingX 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(bingx_symbol: &str) -> String {
        bingx_symbol.replace("-", "/")
    }

    /// Timeframe을 BingX 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BingxTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.event_time.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_qty.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_qty.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.close.as_ref().and_then(|v| v.parse().ok()),
            last: data.close.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.price_change.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.price_change_percent.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &BingxOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let timestamp = Utc::now().timestamp_millis();

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
    fn parse_trade(data: &BingxTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.qty.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

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
            side: if data.buyer_maker.unwrap_or(false) { Some("sell".to_string()) } else { Some("buy".to_string()) },
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
    fn parse_candle(data: &BingxKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let ohlcv = OHLCV {
            timestamp: data.start_time.unwrap_or(0),
            open: data.open.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            high: data.high.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            low: data.low.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            close: data.close.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            volume: data.volume.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // Pong 처리
        if msg == "Pong" {
            return None;
        }

        let response: BingxWsResponse = serde_json::from_str(msg).ok()?;

        let data_type = response.data_type.as_deref()?;
        let symbol = subscribed_symbol.map(Self::to_unified_symbol).unwrap_or_default();

        match data_type {
            s if s.contains("ticker") => {
                if let Some(data) = response.data {
                    if let Ok(ticker_data) = serde_json::from_value::<BingxTickerData>(data) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }
            }
            s if s.contains("depth") => {
                if let Some(data) = response.data {
                    if let Ok(book_data) = serde_json::from_value::<BingxOrderBookData>(data) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, &symbol)));
                    }
                }
            }
            s if s.contains("trade") => {
                if let Some(data) = response.data {
                    if let Ok(trade_data) = serde_json::from_value::<BingxTradeData>(data) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data, &symbol)));
                    }
                }
            }
            s if s.contains("kline") => {
                if let Some(data) = response.data {
                    if let Ok(kline_data) = serde_json::from_value::<BingxKlineData>(data) {
                        let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                        return Some(WsMessage::Ohlcv(Self::parse_candle(&kline_data, &symbol, timeframe)));
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

impl Default for BingxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BingxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": "ticker",
            "reqType": "sub",
            "dataType": format!("{}@ticker", formatted)
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let data_types: Vec<String> = symbols.iter()
            .map(|s| format!("{}@ticker", Self::format_symbol(s)))
            .collect();
        let subscribe_msg = serde_json::json!({
            "id": "tickers",
            "reqType": "sub",
            "dataType": data_types.join(",")
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(20);
        let subscribe_msg = serde_json::json!({
            "id": "depth",
            "reqType": "sub",
            "dataType": format!("{}@depth{}@100ms", formatted, depth)
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "id": "trade",
            "reqType": "sub",
            "dataType": format!("{}@trade", formatted)
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "id": "kline",
            "reqType": "sub",
            "dataType": format!("{}@kline_{}", formatted, interval)
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
#[serde(rename_all = "camelCase")]
struct BingxWsResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    data_type: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxTickerData {
    #[serde(default, rename = "s")]
    symbol: String,
    #[serde(default, rename = "E")]
    event_time: Option<i64>,
    #[serde(default, rename = "c")]
    close: Option<String>,
    #[serde(default, rename = "o")]
    open: Option<String>,
    #[serde(default, rename = "h")]
    high: Option<String>,
    #[serde(default, rename = "l")]
    low: Option<String>,
    #[serde(default, rename = "v")]
    volume: Option<String>,
    #[serde(default, rename = "q")]
    quote_volume: Option<String>,
    #[serde(default, rename = "p")]
    price_change: Option<String>,
    #[serde(default, rename = "P")]
    price_change_percent: Option<String>,
    #[serde(default, rename = "b")]
    bid_price: Option<String>,
    #[serde(default, rename = "B")]
    bid_qty: Option<String>,
    #[serde(default, rename = "a")]
    ask_price: Option<String>,
    #[serde(default, rename = "A")]
    ask_qty: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxTradeData {
    #[serde(default, rename = "t")]
    trade_id: Option<String>,
    #[serde(default, rename = "T")]
    time: Option<i64>,
    #[serde(default, rename = "p")]
    price: Option<String>,
    #[serde(default, rename = "q")]
    qty: Option<String>,
    #[serde(default, rename = "m")]
    buyer_maker: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxKlineData {
    #[serde(default, rename = "t")]
    start_time: Option<i64>,
    #[serde(default, rename = "o")]
    open: Option<String>,
    #[serde(default, rename = "h")]
    high: Option<String>,
    #[serde(default, rename = "l")]
    low: Option<String>,
    #[serde(default, rename = "c")]
    close: Option<String>,
    #[serde(default, rename = "v")]
    volume: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BingxWs::format_symbol("BTC/USDT"), "BTC-USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BingxWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
    }
}
