//! CEX.IO WebSocket Implementation
//!
//! CEX.IO 실시간 데이터 스트리밍

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV, WsExchange, WsMessage,
    WsOhlcvEvent, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://ws.cex.io/ws/";

/// CEX.IO WebSocket 클라이언트
pub struct CexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl CexWs {
    /// 새 CEX.IO WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 CEX.IO 형식으로 변환 (BTC/USD -> BTC-USD)
    fn format_symbol_dash(symbol: &str) -> String {
        symbol.replace('/', "-")
    }

    /// 심볼을 CEX.IO 형식으로 변환 (BTC/USD -> BTC:USD)
    fn format_symbol_colon(symbol: &str) -> String {
        symbol.replace('/', ":")
    }

    /// CEX.IO 심볼을 통합 심볼로 변환 (BTC-USD -> BTC/USD, BTC:USD -> BTC/USD)
    fn to_unified_symbol(cex_symbol: &str) -> String {
        cex_symbol.replace(['-', ':'], "/")
    }

    /// Timeframe을 CEX interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Day1 => "1d",
            _ => "1m", // Default for unsupported timeframes
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CexTickerData, symbol: &str) -> WsTickerEvent {
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.best_bid,
            bid_volume: None,
            ask: data.best_ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.price_change,
            percentage: data.price_change_percentage,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: symbol.to_string(),
            ticker,
        }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &CexOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let mut bids = Vec::new();
        if let Some(bid_array) = &data.bids {
            for bid in bid_array {
                if bid.len() >= 2 {
                    if let (Some(price_val), Some(amount_val)) = (bid.first(), bid.get(1)) {
                        if let (Some(price_str), Some(amount_str)) =
                            (price_val.as_str(), amount_val.as_str())
                        {
                            if let (Ok(price), Ok(amount)) = (
                                price_str.parse::<Decimal>(),
                                amount_str.parse::<Decimal>(),
                            ) {
                                bids.push(OrderBookEntry { price, amount });
                            }
                        }
                    }
                }
            }
        }

        let mut asks = Vec::new();
        if let Some(ask_array) = &data.asks {
            for ask in ask_array {
                if ask.len() >= 2 {
                    if let (Some(price_val), Some(amount_val)) = (ask.first(), ask.get(1)) {
                        if let (Some(price_str), Some(amount_str)) =
                            (price_val.as_str(), amount_val.as_str())
                        {
                            if let (Ok(price), Ok(amount)) = (
                                price_str.parse::<Decimal>(),
                                amount_str.parse::<Decimal>(),
                            ) {
                                asks.push(OrderBookEntry { price, amount });
                            }
                        }
                    }
                }
            }
        }

        let timestamp = data.timestamp.as_ref().and_then(|s| s.parse::<i64>().ok());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
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
    fn parse_trade(data: &CexTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data
            .date_iso
            .as_ref()
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side_str = data
            .side
            .as_ref()
            .map(|s| match s.to_lowercase().as_str() {
                "buy" => "buy",
                "sell" => "sell",
                _ => "buy",
            })
            .unwrap_or("buy");

        let trade = Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: data.date_iso.clone(),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side_str.to_string()),
            taker_or_maker: None,
            price: data.price.unwrap_or(Decimal::ZERO),
            amount: data.amount.unwrap_or(Decimal::ZERO),
            cost: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol: symbol.to_string(),
            trades: vec![trade],
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &CexOhlcvData, timeframe: Timeframe, symbol: &str) -> WsOhlcvEvent {
        let ohlcv = OHLCV {
            timestamp: data.timestamp.unwrap_or(0),
            open: data.open.unwrap_or(Decimal::ZERO),
            high: data.high.unwrap_or(Decimal::ZERO),
            low: data.low.unwrap_or(Decimal::ZERO),
            close: data.close.unwrap_or(Decimal::ZERO),
            volume: data.volume.unwrap_or(Decimal::ZERO),
        };

        WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // JSON 파싱
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // 이벤트 타입 확인
        let event_type = json.get("e").and_then(|v| v.as_str())?;

        match event_type {
            // 티커 업데이트
            "tick" => {
                if let Ok(msg_data) = serde_json::from_str::<CexTickerMessage>(msg) {
                    if let Some(ref data) = msg_data.data {
                        let symbol = Self::to_unified_symbol(
                            data.pair.as_deref().unwrap_or(""),
                        );
                        return Some(WsMessage::Ticker(Self::parse_ticker(data, &symbol)));
                    }
                }
            }
            // 호가창 업데이트
            "md_update" | "orderbook-update" => {
                if let Ok(msg_data) = serde_json::from_str::<CexOrderBookMessage>(msg) {
                    if let Some(ref data) = msg_data.data {
                        let symbol = Self::to_unified_symbol(
                            data.pair.as_deref().unwrap_or(""),
                        );
                        return Some(WsMessage::OrderBook(Self::parse_order_book(data, &symbol)));
                    }
                }
            }
            // 체결 내역
            "md_trade_history" | "trade-update" => {
                if let Ok(msg_data) = serde_json::from_str::<CexTradeMessage>(msg) {
                    if let Some(ref data) = msg_data.data {
                        let symbol = Self::to_unified_symbol(
                            data.pair.as_deref().unwrap_or(""),
                        );
                        return Some(WsMessage::Trade(Self::parse_trade(data, &symbol)));
                    }
                }
            }
            // OHLCV 업데이트
            "ohlcv_update" | "ohlcv-update" => {
                if let Ok(msg_data) = serde_json::from_str::<CexOhlcvMessage>(msg) {
                    if let Some(ref data) = msg_data.data {
                        let symbol = Self::to_unified_symbol(
                            data.pair.as_deref().unwrap_or(""),
                        );
                        // Extract timeframe from interval
                        let timeframe = match data.i.as_deref() {
                            Some("1m") => Timeframe::Minute1,
                            Some("5m") => Timeframe::Minute5,
                            Some("15m") => Timeframe::Minute15,
                            Some("30m") => Timeframe::Minute30,
                            Some("1h") => Timeframe::Hour1,
                            Some("2h") => Timeframe::Hour2,
                            Some("4h") => Timeframe::Hour4,
                            Some("1d") => Timeframe::Day1,
                            _ => Timeframe::Minute1,
                        };
                        return Some(WsMessage::Ohlcv(Self::parse_ohlcv(
                            data, timeframe, &symbol,
                        )));
                    }
                }
            }
            // 연결 확인
            "connected" => return Some(WsMessage::Connected),
            // 구독 확인
            "subscribe" | "subscribed" => {
                if let Some(rooms) = json.get("rooms").and_then(|v| v.as_array()) {
                    if let Some(room) = rooms.first().and_then(|r| r.as_str()) {
                        return Some(WsMessage::Subscribed {
                            channel: event_type.to_string(),
                            symbol: Some(Self::to_unified_symbol(room)),
                        });
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
        symbol: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_BASE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // 구독 메시지 전송
        if let Some(client) = &self.ws_client {
            let subscribe_str = serde_json::to_string(&subscribe_msg).map_err(|e| {
                CcxtError::ExchangeError {
                    message: format!("Failed to serialize subscribe message: {e}"),
                }
            })?;
            client.send(&subscribe_str)?;
        }

        // 구독 저장
        {
            let key = format!("{channel}:{symbol}");
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
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

impl Default for CexWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CexWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }
}

#[async_trait]
impl WsExchange for CexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let pair = Self::format_symbol_dash(symbol);

        let subscribe_msg = serde_json::json!({
            "e": "subscribe",
            "rooms": [format!("pair-{}", pair)]
        });

        client
            .subscribe_stream(subscribe_msg, "ticker", symbol)
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let depth = limit.unwrap_or(0); // 0 = full depth

        let subscribe_msg = serde_json::json!({
            "e": "order-book-subscribe",
            "data": {
                "pair": [parts[0], parts[1]],
                "subscribe": true,
                "depth": depth
            }
        });

        client
            .subscribe_stream(subscribe_msg, "orderBook", symbol)
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let pair = Self::format_symbol_colon(symbol);

        let subscribe_msg = serde_json::json!({
            "e": "md_trade_history",
            "data": {
                "pair": pair
            }
        });

        client
            .subscribe_stream(subscribe_msg, "trades", symbol)
            .await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let pair = Self::format_symbol_dash(symbol);
        let interval = Self::format_interval(timeframe);

        let subscribe_msg = serde_json::json!({
            "e": "init-ohlcv",
            "i": interval,
            "rooms": [format!("pair-{}", pair)]
        });

        client
            .subscribe_stream(subscribe_msg, "ohlcv", symbol)
            .await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // CEX.IO는 구독시 자동 연결
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

// === CEX.IO WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct CexTickerMessage {
    e: String,
    data: Option<CexTickerData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CexTickerData {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default, rename = "bestBid")]
    best_bid: Option<Decimal>,
    #[serde(default, rename = "bestAsk")]
    best_ask: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default, rename = "quoteVolume")]
    quote_volume: Option<Decimal>,
    #[serde(default, rename = "priceChange")]
    price_change: Option<Decimal>,
    #[serde(default, rename = "priceChangePercentage")]
    price_change_percentage: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CexOrderBookMessage {
    e: String,
    data: Option<CexOrderBookData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CexOrderBookData {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    bids: Option<Vec<Vec<serde_json::Value>>>,
    #[serde(default)]
    asks: Option<Vec<Vec<serde_json::Value>>>,
}

#[derive(Debug, Deserialize)]
struct CexTradeMessage {
    e: String,
    data: Option<CexTradeData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CexTradeData {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default, rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(default, rename = "dateISO")]
    date_iso: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CexOhlcvMessage {
    e: String,
    data: Option<CexOhlcvData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CexOhlcvData {
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    i: Option<String>, // interval
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol_dash() {
        assert_eq!(CexWs::format_symbol_dash("BTC/USD"), "BTC-USD");
        assert_eq!(CexWs::format_symbol_dash("ETH/USDT"), "ETH-USDT");
    }

    #[test]
    fn test_format_symbol_colon() {
        assert_eq!(CexWs::format_symbol_colon("BTC/USD"), "BTC:USD");
        assert_eq!(CexWs::format_symbol_colon("ETH/USDT"), "ETH:USDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CexWs::to_unified_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(CexWs::to_unified_symbol("BTC:USD"), "BTC/USD");
        assert_eq!(CexWs::to_unified_symbol("ETH-USDT"), "ETH/USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(CexWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(CexWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(CexWs::format_interval(Timeframe::Day1), "1d");
    }
}
