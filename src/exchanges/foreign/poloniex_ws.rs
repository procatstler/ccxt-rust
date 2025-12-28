//! Poloniex WebSocket Implementation
//!
//! Poloniex 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws.poloniex.com/ws/public";
const WS_PRIVATE_URL: &str = "wss://ws.poloniex.com/ws/private";

/// Poloniex WebSocket 클라이언트
pub struct PoloniexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl PoloniexWs {
    /// 새 Poloniex WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Poloniex 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Poloniex 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(poloniex_symbol: &str) -> String {
        poloniex_symbol.replace("_", "/")
    }

    /// Timeframe을 Poloniex 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "MINUTE_1",
            Timeframe::Minute5 => "MINUTE_5",
            Timeframe::Minute15 => "MINUTE_15",
            Timeframe::Minute30 => "MINUTE_30",
            Timeframe::Hour1 => "HOUR_1",
            Timeframe::Hour2 => "HOUR_2",
            Timeframe::Hour4 => "HOUR_4",
            Timeframe::Hour6 => "HOUR_6",
            Timeframe::Hour12 => "HOUR_12",
            Timeframe::Day1 => "DAY_1",
            Timeframe::Day3 => "DAY_3",
            Timeframe::Week1 => "WEEK_1",
            _ => "MINUTE_1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &PoloniexTickerData) -> WsTickerEvent {
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
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_quantity.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_quantity.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.close.as_ref().and_then(|v| v.parse().ok()),
            last: data.close.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.quantity.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.amount.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: data.mark_price.as_ref().and_then(|v| v.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &PoloniexOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

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
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &PoloniexTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.quantity.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
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

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &PoloniexCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.ts?;
        let ohlcv = OHLCV::new(
            timestamp,
            data.open.parse().ok()?,
            data.high.parse().ok()?,
            data.low.parse().ok()?,
            data.close.parse().ok()?,
            data.quantity.parse().ok()?,
        );

        let symbol = Self::to_unified_symbol(&data.symbol);

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<PoloniexWsResponse>(msg) {
            // Event handling
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response.channel.clone().unwrap_or_default().into_iter().next().unwrap_or_default(),
                        symbol: response.symbols.clone().and_then(|s| s.into_iter().next()),
                    });
                }
            }

            // Data handling
            if let (Some(channel), Some(data)) = (&response.channel, &response.data) {
                let channel_name = channel.get(0).map(|s| s.as_str()).unwrap_or("");

                // Ticker
                if channel_name == "ticker" {
                    if let Ok(ticker_data) = serde_json::from_value::<PoloniexTickerData>(data.clone()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }

                // OrderBook
                if channel_name == "book_lv2" {
                    if let Ok(ob_data) = serde_json::from_value::<PoloniexOrderBookData>(data.clone()) {
                        let symbol = ob_data.symbol.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let is_snapshot = response.event.as_deref() == Some("snapshot");
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                    }
                }

                // Trades
                if channel_name == "trades" {
                    if let Ok(trade_data) = serde_json::from_value::<PoloniexTradeData>(data.clone()) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                    }
                }

                // Candles
                if channel_name.starts_with("candles_") {
                    if let Ok(candle_data) = serde_json::from_value::<PoloniexCandleData>(data.clone()) {
                        if let Some(tf) = timeframe {
                            if let Some(event) = Self::parse_candle(&candle_data, tf) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, channel: &str, symbols: Vec<String>, timeframe: Option<Timeframe>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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
        self.ws_client = Some(ws_client);

        // 구독 메시지 전송
        let subscribe_msg = PoloniexSubscribeMessage {
            event: "subscribe".to_string(),
            channel: vec![channel.to_string()],
            symbols,
        };

        let subscribe_json = serde_json::to_string(&subscribe_msg)
            .map_err(|e| CcxtError::ParseError {
                data_type: "PoloniexSubscribeMessage".to_string(),
                message: e.to_string(),
            })?;

        if let Some(ws_client) = &mut self.ws_client {
            ws_client.send(&subscribe_json)?;
        }

        // 구독 저장
        {
            let key = format!("{channel}:{}", subscribe_msg.symbols.join(","));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 메시지 핸들러
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_message(&msg, timeframe) {
                            let _ = event_tx_clone.send(ws_msg);
                        }
                    },
                    WsEvent::Connected => {
                        let _ = event_tx_clone.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = event_tx_clone.send(WsMessage::Disconnected);
                    },
                    WsEvent::Error(e) => {
                        let _ = event_tx_clone.send(WsMessage::Error(e));
                    },
                    _ => {}
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for PoloniexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for PoloniexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let mut ws = Self::new();
        ws.subscribe_stream("ticker", vec![poloniex_symbol], None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let mut ws = Self::new();
        ws.subscribe_stream("book_lv2", vec![poloniex_symbol], None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let mut ws = Self::new();
        ws.subscribe_stream("trades", vec![poloniex_symbol], None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let poloniex_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let channel = format!("candles_{}", interval.to_lowercase());
        let mut ws = Self::new();
        ws.subscribe_stream(&channel, vec![poloniex_symbol], Some(timeframe)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = self.ws_client.take() {
            ws_client.close()?;
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match self.ws_client.as_ref() {
            Some(ws) => ws.is_connected().await,
            None => false,
        }
    }
}

// WebSocket Message Types

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexSubscribeMessage {
    event: String,
    channel: Vec<String>,
    symbols: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PoloniexWsResponse {
    event: Option<String>,
    channel: Option<Vec<String>>,
    symbols: Option<Vec<String>>,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexTickerData {
    symbol: String,
    #[serde(rename = "dailyChange")]
    daily_change: Option<String>,
    high: Option<String>,
    low: Option<String>,
    amount: Option<String>,
    quantity: Option<String>,
    #[serde(rename = "tradeCount")]
    trade_count: Option<u64>,
    #[serde(rename = "startTime")]
    start_time: Option<i64>,
    close: Option<String>,
    open: Option<String>,
    ts: Option<i64>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    bid: Option<String>,
    #[serde(rename = "bidQuantity")]
    bid_quantity: Option<String>,
    ask: Option<String>,
    #[serde(rename = "askQuantity")]
    ask_quantity: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PoloniexOrderBookData {
    symbol: Option<String>,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
    #[serde(rename = "lastId")]
    last_id: Option<String>,
    id: Option<String>,
    ts: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PoloniexTradeData {
    symbol: String,
    id: String,
    price: String,
    quantity: String,
    #[serde(rename = "takerSide")]
    side: String,
    #[serde(rename = "createTime")]
    create_time: Option<i64>,
    ts: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PoloniexCandleData {
    symbol: String,
    open: String,
    high: String,
    low: String,
    close: String,
    quantity: String,
    amount: Option<String>,
    #[serde(rename = "tradeCount")]
    trade_count: Option<u64>,
    #[serde(rename = "startTime")]
    start_time: Option<i64>,
    #[serde(rename = "closeTime")]
    close_time: Option<i64>,
    ts: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(PoloniexWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(PoloniexWs::format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(PoloniexWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(PoloniexWs::to_unified_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(PoloniexWs::format_interval(Timeframe::Minute1), "MINUTE_1");
        assert_eq!(PoloniexWs::format_interval(Timeframe::Hour1), "HOUR_1");
        assert_eq!(PoloniexWs::format_interval(Timeframe::Day1), "DAY_1");
    }
}
