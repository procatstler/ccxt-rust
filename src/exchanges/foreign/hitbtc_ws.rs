//! HitBTC WebSocket Implementation
//!
//! HitBTC 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://api.hitbtc.com/api/3/ws/public";
const WS_PRIVATE_URL: &str = "wss://api.hitbtc.com/api/3/ws/trading";

/// HitBTC WebSocket 클라이언트
pub struct HitbtcWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: Arc<AtomicI64>,
}

impl HitbtcWs {
    /// 새 HitBTC WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
        }
    }

    /// 다음 요청 ID 생성
    fn next_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 심볼을 HitBTC 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// HitBTC 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(hitbtc_symbol: &str) -> String {
        // 일반적인 quote 통화 목록
        let quotes = ["USDT", "BUSD", "USDC", "BTC", "ETH", "BNB", "TUSD", "EUR", "USD"];

        for quote in quotes {
            if let Some(base) = hitbtc_symbol.strip_suffix(quote) {
                return format!("{}/{}", base, quote);
            }
        }

        // 변환할 수 없으면 원본 반환
        hitbtc_symbol.to_string()
    }

    /// Timeframe을 HitBTC 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "M1",
            Timeframe::Minute3 => "M3",
            Timeframe::Minute5 => "M5",
            Timeframe::Minute15 => "M15",
            Timeframe::Minute30 => "M30",
            Timeframe::Hour1 => "H1",
            Timeframe::Hour4 => "H4",
            Timeframe::Day1 => "D1",
            Timeframe::Week1 => "D7",
            Timeframe::Month1 => "1M",
            _ => "M1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &HitbtcTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.best_bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|v| v.parse().ok()),
            ask: data.best_ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.volume_quote.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &HitbtcOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data.bid.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: b.price.parse().ok()?,
                amount: b.size.parse().ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = data.ask.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: a.price.parse().ok()?,
                amount: a.size.parse().ok()?,
            })
        }).collect();

        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
            nonce: data.sequence,
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
    fn parse_trade(data: &HitbtcTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.quantity.parse().unwrap_or_default();

        let trade = Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
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
        };

        WsTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &HitbtcCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.timestamp.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())?;

        let ohlcv = OHLCV::new(
            timestamp,
            data.open.parse().ok()?,
            data.max.parse().ok()?,
            data.min.parse().ok()?,
            data.close.parse().ok()?,
            data.volume.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
        );

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // JSON-RPC 응답 파싱
        if let Ok(response) = serde_json::from_str::<HitbtcWsResponse>(msg) {
            // 에러 처리
            if let Some(error) = response.error {
                return Some(WsMessage::Error(
                    format!("{}: {}", error.code, error.message)
                ));
            }

            // 결과 처리
            if let Some(result) = response.result {
                // Ticker
                if let Ok(ticker_data) = serde_json::from_value::<HitbtcTickerData>(result.clone()) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                }

                // OrderBook
                if let Ok(ob_data) = serde_json::from_value::<HitbtcOrderBookData>(result.clone()) {
                    let symbol = Self::to_unified_symbol(&ob_data.symbol);
                    return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol)));
                }

                // Trade
                if let Ok(trade_data) = serde_json::from_value::<HitbtcTradeData>(result.clone()) {
                    return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                }

                // Candle
                if let Ok(candle_data) = serde_json::from_value::<HitbtcCandleData>(result.clone()) {
                    // 기본 timeframe (실제로는 구독 시 저장한 값 사용)
                    let timeframe = Timeframe::Minute1;
                    if let Some(event) = Self::parse_candle(&candle_data, timeframe) {
                        return Some(WsMessage::Ohlcv(event));
                    }
                }
            }

            // 채널 업데이트 처리
            if let Some(ch) = response.ch {
                if let Some(data) = response.data {
                    // Ticker update
                    if ch.starts_with("ticker/") {
                        if let Ok(ticker_data) = serde_json::from_value::<HitbtcTickerData>(data.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }

                    // Trades update
                    if ch == "trades" {
                        if let Ok(trade_data) = serde_json::from_value::<HitbtcTradeData>(data.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                        }
                    }

                    // OrderBook update
                    if ch.starts_with("orderbook/") {
                        if let Ok(ob_data) = serde_json::from_value::<HitbtcOrderBookData>(data.clone()) {
                            let symbol = Self::to_unified_symbol(&ob_data.symbol);
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol)));
                        }
                    }

                    // Candles update
                    if ch.starts_with("candles/") {
                        if let Ok(candle_data) = serde_json::from_value::<HitbtcCandleData>(data.clone()) {
                            // Extract timeframe from channel name
                            let timeframe = if let Some(period) = ch.strip_prefix("candles/") {
                                match period {
                                    "M1" => Timeframe::Minute1,
                                    "M3" => Timeframe::Minute3,
                                    "M5" => Timeframe::Minute5,
                                    "M15" => Timeframe::Minute15,
                                    "M30" => Timeframe::Minute30,
                                    "H1" => Timeframe::Hour1,
                                    "H4" => Timeframe::Hour4,
                                    "D1" => Timeframe::Day1,
                                    "D7" => Timeframe::Week1,
                                    "1M" => Timeframe::Month1,
                                    _ => Timeframe::Minute1,
                                }
                            } else {
                                Timeframe::Minute1
                            };

                            if let Some(event) = Self::parse_candle(&candle_data, timeframe) {
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
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbols: Vec<String>,
        channel_name: &str,
        symbol: Option<&str>,
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
        self.ws_client = Some(ws_client);

        // 구독 메시지 전송
        let subscribe_msg = HitbtcSubscribe {
            method: "subscribe".to_string(),
            ch: channel.to_string(),
            params: HitbtcSubscribeParams {
                symbols: Some(symbols.clone()),
            },
            id: self.next_id(),
        };

        let subscribe_json = serde_json::to_string(&subscribe_msg)
            .map_err(|e| CcxtError::ParseError {
                data_type: "HitbtcSubscribe".to_string(),
                message: e.to_string(),
            })?;

        // 구독 저장
        {
            let key = format!("{}:{}", channel_name, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // WebSocket으로 구독 메시지 전송
        if let Some(ws_client) = &self.ws_client {
            ws_client.send(&subscribe_json)?;
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

impl Default for HitbtcWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HitbtcWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(AtomicI64::new(1)),
        }
    }
}

#[async_trait]
impl WsExchange for HitbtcWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        client.subscribe_stream("ticker/1s", vec![market_symbol], "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbols: Vec<String> = symbols
            .iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream("ticker/1s", market_symbols, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        client.subscribe_stream("orderbook/full", vec![market_symbol], "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        client.subscribe_stream("trades", vec![market_symbol], "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let channel = format!("candles/{}", interval);
        client.subscribe_stream(&channel, vec![market_symbol], "ohlcv", Some(symbol)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // HitBTC는 구독시 자동 연결
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

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        Err(CcxtError::NotSupported {
            feature: "ws_authenticate - Private WebSocket not implemented yet".to_string(),
        })
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_balance - Private WebSocket not implemented yet".to_string(),
        })
    }

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_orders - Private WebSocket not implemented yet".to_string(),
        })
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(CcxtError::NotSupported {
            feature: "watch_my_trades - Private WebSocket not implemented yet".to_string(),
        })
    }
}

// === HitBTC WebSocket Message Types ===

#[derive(Debug, Serialize)]
struct HitbtcSubscribe {
    method: String,
    ch: String,
    params: HitbtcSubscribeParams,
    id: i64,
}

#[derive(Debug, Serialize)]
struct HitbtcSubscribeParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    symbols: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct HitbtcWsResponse {
    #[serde(default)]
    ch: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<HitbtcError>,
    #[serde(default)]
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HitbtcError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HitbtcTickerData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_quote: Option<String>,
    #[serde(default, rename = "best_bid_price")]
    best_bid_price: Option<String>,
    #[serde(default, rename = "best_bid_size")]
    best_bid_size: Option<String>,
    #[serde(default, rename = "best_ask_price")]
    best_ask_price: Option<String>,
    #[serde(default, rename = "best_ask_size")]
    best_ask_size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HitbtcOrderBookLevel {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct HitbtcOrderBookData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    sequence: Option<i64>,
    #[serde(default)]
    ask: Vec<HitbtcOrderBookLevel>,
    #[serde(default)]
    bid: Vec<HitbtcOrderBookLevel>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HitbtcTradeData {
    #[serde(default)]
    id: i64,
    symbol: String,
    price: String,
    quantity: String,
    side: String,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HitbtcCandleData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    open: String,
    close: String,
    min: String,
    max: String,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_quote: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(HitbtcWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(HitbtcWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(HitbtcWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(HitbtcWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(HitbtcWs::format_interval(Timeframe::Minute1), "M1");
        assert_eq!(HitbtcWs::format_interval(Timeframe::Hour1), "H1");
        assert_eq!(HitbtcWs::format_interval(Timeframe::Day1), "D1");
    }
}
