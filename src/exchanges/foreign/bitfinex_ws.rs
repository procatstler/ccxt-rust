//! Bitfinex WebSocket Implementation
//!
//! Bitfinex 실시간 데이터 스트리밍 (Public Streams)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV, WsExchange, WsMessage,
    WsOhlcvEvent, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://api-pub.bitfinex.com/ws/2";

/// Bitfinex WebSocket 클라이언트
pub struct BitfinexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    /// 채널 ID → (채널명, 심볼) 매핑
    channel_map: Arc<RwLock<HashMap<i64, (String, String)>>>,
}

impl BitfinexWs {
    /// 새 Bitfinex WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            channel_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// API 키와 시크릿으로 Bitfinex WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            channel_map: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 심볼을 Bitfinex 형식으로 변환 (BTC/USD -> tBTCUSD)
    fn format_symbol(symbol: &str) -> String {
        format!("t{}", symbol.replace("/", ""))
    }

    /// Bitfinex 심볼을 통합 심볼로 변환 (tBTCUSD -> BTC/USD)
    fn to_unified_symbol(bitfinex_symbol: &str) -> String {
        if !bitfinex_symbol.starts_with('t') {
            return bitfinex_symbol.to_string();
        }

        let s = &bitfinex_symbol[1..];

        // Try common quote currencies
        let quotes = ["USDT", "USD", "USDC", "BTC", "ETH", "EUR", "GBP", "JPY"];

        for quote in quotes {
            if let Some(base) = s.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        // If no match, try 3/3 split
        if s.len() >= 6 {
            let base = &s[..3];
            let quote = &s[3..];
            return format!("{base}/{quote}");
        }

        bitfinex_symbol.to_string()
    }

    /// Timeframe을 Bitfinex 형식으로 변환
    fn format_timeframe(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m", // Default to 1m for unsupported timeframes
        }
    }

    /// Parse decimal from JSON value (handles both string and number)
    fn parse_decimal(value: &serde_json::Value) -> Option<Decimal> {
        value.as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| value.as_f64().and_then(|f| Decimal::try_from(f).ok()))
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &[serde_json::Value], symbol: &str) -> Option<WsTickerEvent> {
        // Ticker format: [BID, BID_SIZE, ASK, ASK_SIZE, DAILY_CHANGE, DAILY_CHANGE_RELATIVE,
        //                 LAST_PRICE, VOLUME, HIGH, LOW]
        if data.len() < 10 {
            return None;
        }

        let bid = Self::parse_decimal(&data[0])?;
        let ask = Self::parse_decimal(&data[2])?;
        let last = Self::parse_decimal(&data[6])?;
        let volume = Self::parse_decimal(&data[7])?;
        let high = Self::parse_decimal(&data[8])?;
        let low = Self::parse_decimal(&data[9])?;
        let change = Self::parse_decimal(&data[4])?;
        let percentage = Self::parse_decimal(&data[5])?;

        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: Some(high),
            low: Some(low),
            bid: Some(bid),
            bid_volume: Self::parse_decimal(&data[1]),
            ask: Some(ask),
            ask_volume: Self::parse_decimal(&data[3]),
            vwap: None,
            open: None,
            close: Some(last),
            last: Some(last),
            previous_close: None,
            change: Some(change),
            percentage: Some(percentage * Decimal::from(100)),
            average: None,
            base_volume: Some(volume),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTickerEvent {
            symbol: symbol.to_string(),
            ticker,
        })
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &[serde_json::Value], symbol: &str, is_snapshot: bool) -> Option<WsOrderBookEvent> {
        let timestamp = Utc::now().timestamp_millis();
        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if is_snapshot {
            // Snapshot: array of [PRICE, COUNT, AMOUNT]
            for entry in data {
                if let Some(arr) = entry.as_array() {
                    if arr.len() >= 3 {
                        let price = Self::parse_decimal(&arr[0])?;
                        let count: i64 = arr[1].as_i64()?;
                        let amount = Self::parse_decimal(&arr[2])?;

                        if count > 0 {
                            let entry = OrderBookEntry {
                                price,
                                amount: amount.abs(),
                            };

                            if amount > Decimal::ZERO {
                                bids.push(entry);
                            } else {
                                asks.push(entry);
                            }
                        }
                    }
                }
            }
        } else {
            // Update: [PRICE, COUNT, AMOUNT]
            if data.len() >= 3 {
                let price = Self::parse_decimal(&data[0])?;
                let count: i64 = data[1].as_i64()?;
                let amount = Self::parse_decimal(&data[2])?;

                if count > 0 {
                    let entry = OrderBookEntry {
                        price,
                        amount: amount.abs(),
                    };

                    if amount > Decimal::ZERO {
                        bids.push(entry);
                    } else {
                        asks.push(entry);
                    }
                }
            }
        }

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        };

        Some(WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        })
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &[serde_json::Value], symbol: &str) -> Option<WsTradeEvent> {
        // Trade format: [ID, MTS, AMOUNT, PRICE]
        // Or array of trades for snapshot
        let mut trades = Vec::new();

        if data.is_empty() {
            return None;
        }

        // Check if first element is an array (snapshot)
        if data[0].is_array() {
            for trade_data in data {
                if let Some(arr) = trade_data.as_array() {
                    if let Some(trade) = Self::parse_single_trade(arr, symbol) {
                        trades.push(trade);
                    }
                }
            }
        } else {
            // Single trade update
            if let Some(trade) = Self::parse_single_trade(data, symbol) {
                trades.push(trade);
            }
        }

        if trades.is_empty() {
            None
        } else {
            Some(WsTradeEvent {
                symbol: symbol.to_string(),
                trades,
            })
        }
    }

    /// 단일 체결 파싱
    fn parse_single_trade(data: &[serde_json::Value], symbol: &str) -> Option<Trade> {
        if data.len() < 4 {
            return None;
        }

        let id: i64 = data[0].as_i64()?;
        let timestamp: i64 = data[1].as_i64()?;
        let amount = Self::parse_decimal(&data[2])?;
        let price = Self::parse_decimal(&data[3])?;

        let side = if amount > Decimal::ZERO { "buy" } else { "sell" };
        let amount_abs = amount.abs();

        Some(Trade {
            id: id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount: amount_abs,
            cost: Some(price * amount_abs),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Candle 메시지 파싱
    fn parse_candle(data: &[serde_json::Value], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        // Candle format: [MTS, OPEN, CLOSE, HIGH, LOW, VOLUME]
        if data.len() < 6 {
            return None;
        }

        let timestamp: i64 = data[0].as_i64()?;
        let open = Self::parse_decimal(&data[1])?;
        let close = Self::parse_decimal(&data[2])?;
        let high = Self::parse_decimal(&data[3])?;
        let low = Self::parse_decimal(&data[4])?;
        let volume = Self::parse_decimal(&data[5])?;

        let ohlcv = OHLCV {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
        };

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str, channel_map: Arc<RwLock<HashMap<i64, (String, String)>>>) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Handle event messages
        if let Some(event) = json.get("event").and_then(|v| v.as_str()) {
            match event {
                "subscribed" => {
                    let chan_id = json.get("chanId")?.as_i64()?;
                    let channel = json.get("channel")?.as_str()?;
                    let symbol = json.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                    let pair = json.get("pair").and_then(|v| v.as_str()).unwrap_or("");
                    let key = json.get("key").and_then(|v| v.as_str()).unwrap_or("");

                    // Store channel mapping
                    let symbol_value = if !symbol.is_empty() {
                        Self::to_unified_symbol(symbol)
                    } else if !pair.is_empty() {
                        Self::to_unified_symbol(&format!("t{}", pair))
                    } else if !key.is_empty() && key.starts_with("trade:") {
                        // Extract symbol from key like "trade:1m:tBTCUSD"
                        if let Some(sym) = key.split(':').nth(2) {
                            Self::to_unified_symbol(sym)
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            channel_map.write().await.insert(chan_id, (channel.to_string(), symbol_value.clone()));
                        })
                    });

                    return Some(WsMessage::Subscribed {
                        channel: channel.to_string(),
                        symbol: if symbol_value.is_empty() { None } else { Some(symbol_value) },
                    });
                }
                "info" | "conf" => {
                    // Informational messages, ignore
                    return None;
                }
                "error" => {
                    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("Unknown error");
                    return Some(WsMessage::Error(msg.to_string()));
                }
                _ => return None,
            }
        }

        // Handle data messages [CHAN_ID, DATA] or [CHAN_ID, "hb"] for heartbeat
        if let Some(arr) = json.as_array() {
            if arr.len() >= 2 {
                let chan_id = arr[0].as_i64()?;

                // Heartbeat
                if arr[1].as_str() == Some("hb") {
                    return None;
                }

                // Get channel info
                let (channel, symbol) = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        channel_map.read().await.get(&chan_id).cloned()
                    })
                })?;

                // Parse based on channel type
                match channel.as_str() {
                    "ticker" => {
                        if let Some(data) = arr[1].as_array() {
                            return Self::parse_ticker(data, &symbol).map(WsMessage::Ticker);
                        }
                    }
                    "book" => {
                        // Check if snapshot or update
                        let is_snapshot = arr[1].is_array() && arr[1].as_array()?.first()?.is_array();

                        if is_snapshot {
                            if let Some(data) = arr[1].as_array() {
                                return Self::parse_order_book(data, &symbol, true).map(WsMessage::OrderBook);
                            }
                        } else {
                            if let Some(data) = arr[1].as_array() {
                                return Self::parse_order_book(data, &symbol, false).map(WsMessage::OrderBook);
                            }
                        }
                    }
                    "trades" => {
                        if let Some(data_wrapper) = arr[1].as_array() {
                            // Check for snapshot or update
                            if data_wrapper.len() >= 2 {
                                // Update: ["te" or "tu", [trade_data]]
                                if let Some(trade_data) = data_wrapper[1].as_array() {
                                    return Self::parse_trade(trade_data, &symbol).map(WsMessage::Trade);
                                }
                            } else if !data_wrapper.is_empty() {
                                // Snapshot: [[trade1], [trade2], ...]
                                return Self::parse_trade(data_wrapper, &symbol).map(WsMessage::Trade);
                            }
                        }
                    }
                    "candles" => {
                        if let Some(data) = arr[1].as_array() {
                            // Determine timeframe from channel info (stored in symbol field)
                            // For candles, we need to extract timeframe from the key
                            let timeframe = Timeframe::Minute1; // Default, should be stored in channel_map
                            return Self::parse_candle(data, &symbol, timeframe).map(WsMessage::Ohlcv);
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbol: &str,
        params: Option<serde_json::Value>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket URL
        let url = WS_BASE_URL.to_string();

        // WebSocket 클라이언트 생성 및 연결
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

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol);
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Send subscription message
        let market_id = Self::format_symbol(symbol);
        let mut sub_msg = serde_json::json!({
            "event": "subscribe",
            "channel": channel,
        });

        if let Some(p) = params {
            sub_msg.as_object_mut().unwrap().extend(p.as_object().unwrap().clone());
        } else {
            // Add symbol for ticker, book, trades
            if channel != "candles" {
                sub_msg["symbol"] = serde_json::json!(market_id);
            }
        }

        if let Some(ws) = &self.ws_client {
            ws.send(&serde_json::to_string(&sub_msg).unwrap_or_default())?;
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let channel_map = Arc::clone(&self.channel_map);
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
                        if let Some(ws_msg) = Self::process_message(&msg, Arc::clone(&channel_map)) {
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

impl Default for BitfinexWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BitfinexWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            channel_map: Arc::clone(&self.channel_map),
        }
    }
}

#[async_trait]
impl WsExchange for BitfinexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("ticker", symbol, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);

        // Bitfinex book precision levels: P0 (most precise), P1, P2, P3
        let prec = "P0";
        let len = limit.unwrap_or(25).to_string();

        let params = serde_json::json!({
            "symbol": market_id,
            "prec": prec,
            "len": len,
        });

        client.subscribe_stream("book", symbol, Some(params)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("trades", symbol, None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let tf = Self::format_timeframe(timeframe);
        let key = format!("trade:{tf}:{market_id}");

        let params = serde_json::json!({
            "key": key,
        });

        client.subscribe_stream("candles", symbol, Some(params)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Bitfinex는 구독시 자동 연결
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitfinexWs::format_symbol("BTC/USD"), "tBTCUSD");
        assert_eq!(BitfinexWs::format_symbol("ETH/USDT"), "tETHUSDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitfinexWs::to_unified_symbol("tBTCUSD"), "BTC/USD");
        assert_eq!(BitfinexWs::to_unified_symbol("tETHUSDT"), "ETH/USDT");
        assert_eq!(BitfinexWs::to_unified_symbol("tBTCUSDT"), "BTC/USDT");
    }

    #[test]
    fn test_format_timeframe() {
        assert_eq!(BitfinexWs::format_timeframe(Timeframe::Minute1), "1m");
        assert_eq!(BitfinexWs::format_timeframe(Timeframe::Hour1), "1h");
        assert_eq!(BitfinexWs::format_timeframe(Timeframe::Day1), "1D");
    }
}
