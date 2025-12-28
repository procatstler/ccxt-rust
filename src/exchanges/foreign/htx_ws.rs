//! HTX (Huobi) WebSocket Implementation
//!
//! HTX 실시간 데이터 스트리밍

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://api.huobi.pro/ws";
const WS_MARKET_URL: &str = "wss://api.huobi.pro/ws";

/// HTX WebSocket 클라이언트
pub struct HtxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl HtxWs {
    /// 새 HTX WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 HTX 형식으로 변환 (BTC/USDT -> btcusdt)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// HTX 심볼을 통합 심볼로 변환 (btcusdt -> BTC/USDT)
    fn to_unified_symbol(htx_symbol: &str) -> String {
        // Most common quote currencies
        let quote_currencies = ["usdt", "btc", "eth", "husd", "usdc", "trx"];
        let lower = htx_symbol.to_lowercase();

        for quote in &quote_currencies {
            if lower.ends_with(quote) {
                let base = &lower[..lower.len() - quote.len()];
                return format!("{}/{}", base.to_uppercase(), quote.to_uppercase());
            }
        }

        htx_symbol.to_uppercase()
    }

    /// Timeframe을 HTX 포맷으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "60min",
            Timeframe::Hour4 => "4hour",
            Timeframe::Day1 => "1day",
            Timeframe::Week1 => "1week",
            Timeframe::Month1 => "1mon",
            _ => "1min",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &HtxTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.bid.first().copied(),
            bid_volume: data.bid.get(1).copied(),
            ask: data.ask.first().copied(),
            ask_volume: data.ask.get(1).copied(),
            vwap: None,
            open: data.open,
            close: data.close,
            last: data.close,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.amount,
            quote_volume: data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &HtxOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0],
                    amount: b[1],
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0],
                    amount: a[1],
                })
            } else {
                None
            }
        }).collect();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.version,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &HtxTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let trades = vec![Trade {
            id: data.trade_id.map(|id| id.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side: data.direction.clone(),
            taker_or_maker: None,
            price: data.price.unwrap_or_default(),
            amount: data.amount.unwrap_or_default(),
            cost: data.price.and_then(|p| data.amount.map(|a| p * a)),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // HTX sends gzip compressed data, but our WsClient should handle decompression
        // Parse the JSON message
        if let Ok(response) = serde_json::from_str::<HtxWsResponse>(msg) {
            // Ping response - we handle pong internally
            if response.ping.is_some() {
                return None; // Ping/pong handled elsewhere
            }

            // Subscribe confirmation
            if response.subbed.is_some() {
                return Some(WsMessage::Subscribed {
                    channel: response.subbed.unwrap_or_default(),
                    symbol: None,
                });
            }

            // Data message
            if let Some(ch) = &response.ch {
                if let Some(tick) = response.tick {
                    // Parse channel to determine message type
                    if ch.contains(".ticker") {
                        // Extract symbol from channel (market.btcusdt.ticker)
                        let parts: Vec<&str> = ch.split('.').collect();
                        let symbol = parts.get(1).unwrap_or(&"");
                        if let Ok(ticker_data) = serde_json::from_value::<HtxTickerData>(tick) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    } else if ch.contains(".depth") {
                        let parts: Vec<&str> = ch.split('.').collect();
                        let symbol = parts.get(1).unwrap_or(&"");
                        if let Ok(ob_data) = serde_json::from_value::<HtxOrderBookData>(tick) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, symbol, true)));
                        }
                    } else if ch.contains(".trade") {
                        let parts: Vec<&str> = ch.split('.').collect();
                        let symbol = parts.get(1).unwrap_or(&"");
                        if let Ok(trade_wrapper) = serde_json::from_value::<HtxTradeWrapper>(tick) {
                            if let Some(trade_data) = trade_wrapper.data.first() {
                                return Some(WsMessage::Trade(Self::parse_trade(trade_data, symbol)));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        topic: &str,
        channel: &str,
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
            "sub": topic,
            "id": format!("sub_{}", Utc::now().timestamp_millis())
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = topic.to_string();
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
                        // Handle ping from HTX - ping/pong is handled by WsClient
                        // Just process regular messages
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

impl Default for HtxWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for HtxWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let topic = format!("market.{htx_symbol}.ticker");
        client.subscribe_stream(&topic, "ticker").await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // HTX doesn't support batch ticker subscription, subscribe to first symbol
        if let Some(symbol) = symbols.first() {
            self.watch_ticker(symbol).await
        } else {
            Err(crate::errors::CcxtError::BadRequest {
                message: "No symbols provided".into(),
            })
        }
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let depth = match limit.unwrap_or(20) {
            1..=5 => "step0",
            6..=20 => "step1",
            _ => "step2",
        };
        let topic = format!("market.{htx_symbol}.depth.{depth}");
        client.subscribe_stream(&topic, "orderBook").await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let topic = format!("market.{htx_symbol}.trade.detail");
        client.subscribe_stream(&topic, "trades").await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let htx_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("market.{htx_symbol}.kline.{interval}");
        client.subscribe_stream(&topic, "ohlcv").await
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

// === HTX WebSocket Types ===

#[derive(Debug, Deserialize)]
struct HtxPing {
    #[serde(default)]
    ping: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HtxWsResponse {
    #[serde(default)]
    ping: Option<i64>,
    #[serde(default)]
    subbed: Option<String>,
    #[serde(default)]
    ch: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    tick: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HtxTickerData {
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    bid: Vec<Decimal>,
    #[serde(default)]
    ask: Vec<Decimal>,
    #[serde(default)]
    ts: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct HtxOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    version: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct HtxTradeWrapper {
    #[serde(default)]
    data: Vec<HtxTradeData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HtxTradeData {
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(HtxWs::format_symbol("BTC/USDT"), "btcusdt");
        assert_eq!(HtxWs::format_symbol("ETH/BTC"), "ethbtc");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(HtxWs::to_unified_symbol("btcusdt"), "BTC/USDT");
        assert_eq!(HtxWs::to_unified_symbol("ethbtc"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(HtxWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(HtxWs::format_interval(Timeframe::Hour1), "60min");
        assert_eq!(HtxWs::format_interval(Timeframe::Day1), "1day");
    }
}
