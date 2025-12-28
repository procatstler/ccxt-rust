//! OKX WebSocket Implementation
//!
//! OKX 실시간 데이터 스트리밍

#![allow(clippy::manual_strip)]

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

const WS_PUBLIC_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";

/// OKX WebSocket 클라이언트
pub struct OkxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl OkxWs {
    /// 새 OKX WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 OKX 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Timeframe을 OKX bar로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour2 => "2H",
            Timeframe::Hour4 => "4H",
            Timeframe::Hour6 => "6H",
            Timeframe::Hour12 => "12H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &OkxTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_px.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_px.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open24h.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol24h.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_ccy24h.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &OkxOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.seq_id,
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
    fn parse_trade(data: &OkxTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.px.parse().unwrap_or_default();
        let amount: Decimal = data.sz.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone(),
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
    fn parse_candle(data: &[String], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let timestamp = data[0].parse::<i64>().ok()?;
        let ohlcv = OHLCV::new(
            timestamp,
            data[1].parse().ok()?,
            data[2].parse().ok()?,
            data[3].parse().ok()?,
            data[4].parse().ok()?,
            data[5].parse().ok()?,
        );

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// OKX 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(okx_symbol: &str) -> String {
        okx_symbol.replace("-", "/")
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<OkxWsResponse>(msg) {
            // 에러 체크
            if response.code.as_ref().map(|c| c != "0").unwrap_or(false) {
                return Some(WsMessage::Error(response.msg.unwrap_or_default()));
            }

            // 이벤트 처리
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response.arg.as_ref()
                            .and_then(|a| a.channel.clone())
                            .unwrap_or_default(),
                        symbol: response.arg.as_ref().and_then(|a| a.inst_id.clone()),
                    });
                }
            }

            // 데이터 처리
            if let (Some(arg), Some(data_arr)) = (&response.arg, &response.data) {
                let channel = arg.channel.as_deref().unwrap_or("");

                // Ticker
                if channel == "tickers" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<OkxTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }
                }

                // OrderBook
                if channel.starts_with("books") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ob_data) = serde_json::from_value::<OkxOrderBookData>(first.clone()) {
                            let symbol = arg.inst_id.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            let is_snapshot = response.action.as_deref() == Some("snapshot");
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                        }
                    }
                }

                // Trade
                if channel == "trades" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(trade_data) = serde_json::from_value::<OkxTradeData>(first.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }
                }

                // Candle
                if channel.starts_with("candle") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(candle_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let symbol = arg.inst_id.as_ref()
                                .map(|s| Self::to_unified_symbol(s))
                                .unwrap_or_default();
                            // Extract timeframe from channel (e.g., "candle1m")
                            let timeframe = match &channel[6..] {
                                "1m" => Timeframe::Minute1,
                                "3m" => Timeframe::Minute3,
                                "5m" => Timeframe::Minute5,
                                "15m" => Timeframe::Minute15,
                                "30m" => Timeframe::Minute30,
                                "1H" => Timeframe::Hour1,
                                "2H" => Timeframe::Hour2,
                                "4H" => Timeframe::Hour4,
                                "6H" => Timeframe::Hour6,
                                "12H" => Timeframe::Hour12,
                                "1D" => Timeframe::Day1,
                                "1W" => Timeframe::Week1,
                                "1M" => Timeframe::Month1,
                                _ => Timeframe::Minute1,
                            };
                            if let Some(event) = Self::parse_candle(&candle_arr, &symbol, timeframe) {
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
    async fn subscribe_stream(&mut self, args: Vec<serde_json::Value>, channel: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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
            "op": "subscribe",
            "args": args
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
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

impl Default for OkxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for OkxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "channel": "tickers",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args: Vec<serde_json::Value> = symbols
            .iter()
            .map(|s| serde_json::json!({
                "channel": "tickers",
                "instId": Self::format_symbol(s)
            }))
            .collect();
        client.subscribe_stream(args, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = match limit.unwrap_or(400) {
            1 => "bbo-tbt",
            5 => "books5",
            _ => "books",
        };
        let args = vec![serde_json::json!({
            "channel": channel,
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "channel": "trades",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let args = vec![serde_json::json!({
            "channel": format!("candle{}", interval),
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ohlcv", Some(symbol)).await
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

// === OKX WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct OkxWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    arg: Option<OkxWsArg>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OkxWsArg {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    inst_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxTickerData {
    inst_id: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    last_sz: Option<String>,
    #[serde(default)]
    ask_px: Option<String>,
    #[serde(default)]
    ask_sz: Option<String>,
    #[serde(default)]
    bid_px: Option<String>,
    #[serde(default)]
    bid_sz: Option<String>,
    #[serde(default)]
    open24h: Option<String>,
    #[serde(default)]
    high24h: Option<String>,
    #[serde(default)]
    low24h: Option<String>,
    #[serde(default)]
    vol_ccy24h: Option<String>,
    #[serde(default)]
    vol24h: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    seq_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OkxTradeData {
    inst_id: String,
    trade_id: String,
    px: String,
    sz: String,
    side: String,
    #[serde(default)]
    ts: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(OkxWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(OkxWs::format_symbol("ETH/BTC"), "ETH-BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(OkxWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(OkxWs::to_unified_symbol("ETH-BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(OkxWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(OkxWs::format_interval(Timeframe::Hour1), "1H");
        assert_eq!(OkxWs::format_interval(Timeframe::Day1), "1D");
    }
}
