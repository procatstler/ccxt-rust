//! Hyperliquid DEX WebSocket Implementation
//!
//! Hyperliquid 실시간 데이터 스트리밍 (Public Streams)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV, WsExchange, WsMessage,
    WsOhlcvEvent, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://api.hyperliquid.xyz/ws";
const WS_TESTNET_URL: &str = "wss://api.hyperliquid-testnet.xyz/ws";

/// Hyperliquid WebSocket 클라이언트
pub struct HyperliquidWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    sandbox: bool,
}

impl HyperliquidWs {
    /// 새 Hyperliquid WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: false,
        }
    }

    /// Testnet WebSocket 클라이언트 생성
    pub fn new_sandbox() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: true,
        }
    }

    /// Get WebSocket URL
    fn get_ws_url(&self) -> &str {
        if self.sandbox {
            WS_TESTNET_URL
        } else {
            WS_BASE_URL
        }
    }

    /// 심볼을 Hyperliquid coin 형식으로 변환
    /// BTC/USDC:USDC -> BTC (for swaps)
    /// PURR/USDC -> PURR/USDC (for spot)
    fn symbol_to_coin(symbol: &str) -> String {
        if symbol.contains(':') {
            // Swap format: BTC/USDC:USDC -> BTC
            symbol.split('/').next().unwrap_or(symbol).to_string()
        } else {
            // Spot format: keep as is
            symbol.to_string()
        }
    }

    /// Hyperliquid coin을 통합 심볼로 변환
    fn coin_to_symbol(coin: &str, is_swap: bool) -> String {
        if is_swap {
            format!("{coin}/USDC:USDC")
        } else {
            coin.to_string()
        }
    }

    /// Timeframe을 Hyperliquid interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
            _ => "1h", // Default fallback
        }
    }

    /// Parse ticker message (allMids subscription)
    fn parse_ticker_message(data: &HyperliquidAllMids) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        for (coin, mid_price) in &data.mids {
            let symbol = Self::coin_to_symbol(coin, true);

            if let Ok(price) = Decimal::from_str(mid_price) {
                let timestamp = Utc::now().timestamp_millis();

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
                    close: Some(price),
                    last: Some(price),
                    previous_close: None,
                    change: None,
                    percentage: None,
                    average: None,
                    base_volume: None,
                    quote_volume: None,
                    index_price: None,
                    mark_price: Some(price),
                    info: serde_json::to_value(data).unwrap_or_default(),
                };

                messages.push(WsMessage::Ticker(WsTickerEvent { symbol, ticker }));
            }
        }

        messages
    }

    /// Parse order book message (l2Book subscription)
    fn parse_order_book_message(data: &HyperliquidL2Book) -> Option<WsMessage> {
        let coin = &data.coin;
        let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

        let timestamp = data.time.parse::<i64>().ok()
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // Parse bids (first array in levels)
        let bids: Vec<OrderBookEntry> = data.levels.first()
            .map(|level_array| {
                level_array.iter()
                    .filter_map(|level| {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&level.px).ok()?,
                            amount: Decimal::from_str(&level.sz).ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Parse asks (second array in levels)
        let asks: Vec<OrderBookEntry> = data.levels.get(1)
            .map(|level_array| {
                level_array.iter()
                    .filter_map(|level| {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&level.px).ok()?,
                            amount: Decimal::from_str(&level.sz).ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let order_book = OrderBook {
            symbol: symbol.clone(),
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

        Some(WsMessage::OrderBook(WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,
        }))
    }

    /// Parse trades message (trades subscription)
    fn parse_trades_message(data: &[HyperliquidTrade]) -> Option<WsMessage> {
        if data.is_empty() {
            return None;
        }

        let coin = &data[0].coin;
        let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

        let trades: Vec<Trade> = data.iter()
            .filter_map(|t| {
                let price = Decimal::from_str(&t.px).ok()?;
                let amount = Decimal::from_str(&t.sz).ok()?;
                let timestamp = t.time;

                Some(Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side: if t.side == "A" {
                        Some("sell".to_string())
                    } else {
                        Some("buy".to_string())
                    },
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                })
            })
            .collect();

        if trades.is_empty() {
            None
        } else {
            Some(WsMessage::Trade(WsTradeEvent { symbol, trades }))
        }
    }

    /// Parse candle message (candle subscription)
    fn parse_candle_message(data: &HyperliquidCandle, timeframe: Timeframe) -> Option<WsMessage> {
        let coin = &data.s;
        let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

        let ohlcv = OHLCV {
            timestamp: data.t,
            open: Decimal::from_str(&data.o).ok()?,
            high: Decimal::from_str(&data.h).ok()?,
            low: Decimal::from_str(&data.l).ok()?,
            close: Decimal::from_str(&data.c).ok()?,
            volume: Decimal::from_str(&data.v).ok()?,
        };

        Some(WsMessage::Ohlcv(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }))
    }

    /// Process incoming WebSocket message
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        // Parse as generic value first to check channel
        let json: serde_json::Value = match serde_json::from_str(msg) {
            Ok(v) => v,
            Err(_) => return messages,
        };

        let channel = json.get("channel").and_then(|c| c.as_str()).unwrap_or("");

        match channel {
            "allMids" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidAllMidsMsg>(json) {
                    messages.extend(Self::parse_ticker_message(&data.data));
                }
            }
            "l2Book" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidL2BookMsg>(json) {
                    if let Some(msg) = Self::parse_order_book_message(&data.data) {
                        messages.push(msg);
                    }
                }
            }
            "trades" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidTradesMsg>(json) {
                    if let Some(msg) = Self::parse_trades_message(&data.data) {
                        messages.push(msg);
                    }
                }
            }
            "candle" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidCandleMsg>(json) {
                    if let Some(tf) = timeframe {
                        if let Some(msg) = Self::parse_candle_message(&data.data, tf) {
                            messages.push(msg);
                        }
                    }
                }
            }
            "error" => {
                if let Some(error_msg) = json.get("data").and_then(|d| d.as_str()) {
                    messages.push(WsMessage::Error(error_msg.to_string()));
                }
            }
            "subscriptionResponse" => {
                // Subscription confirmation - can be ignored or logged
            }
            _ => {
                // Unknown channel - ignore or log
            }
        }

        messages
    }

    /// Subscribe to a channel and return event receiver
    async fn subscribe_channel(
        &mut self,
        subscription: serde_json::Value,
        channel_key: &str,
        timeframe: Option<Timeframe>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = self.get_ws_url();

        // Create WebSocket client
        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20, // Hyperliquid keepAlive: 20000ms
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send subscription message
        let sub_msg = serde_json::to_string(&subscription)
            .map_err(|e| CcxtError::ParseError {
                data_type: "subscription".to_string(),
                message: e.to_string(),
            })?;

        ws_client.send(&sub_msg)?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            self.subscriptions.write().await.insert(
                channel_key.to_string(),
                serde_json::to_string(&subscription).unwrap_or_default(),
            );
        }

        // Spawn event processing task
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
                        let messages = Self::process_message(&msg, timeframe);
                        for ws_msg in messages {
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

impl Default for HyperliquidWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HyperliquidWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: self.sandbox,
        }
    }
}

#[async_trait]
impl WsExchange for HyperliquidWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "allMids"
            }
        });

        client.subscribe_channel(subscription, &format!("ticker:{symbol}"), None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let coin = Self::symbol_to_coin(symbol);

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "l2Book",
                "coin": coin
            }
        });

        client.subscribe_channel(subscription, &format!("orderBook:{symbol}"), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let coin = Self::symbol_to_coin(symbol);

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "trades",
                "coin": coin
            }
        });

        client.subscribe_channel(subscription, &format!("trades:{symbol}"), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let coin = Self::symbol_to_coin(symbol);
        let interval = Self::format_interval(timeframe);

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "candle",
                "coin": coin,
                "interval": interval
            }
        });

        client.subscribe_channel(subscription, &format!("candle:{symbol}:{interval}"), Some(timeframe)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Hyperliquid auto-connects on subscription
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

// === Hyperliquid WebSocket Message Types ===

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidAllMidsMsg {
    channel: String,
    data: HyperliquidAllMids,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidAllMids {
    #[serde(default)]
    dex: Option<String>,
    mids: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidL2BookMsg {
    channel: String,
    data: HyperliquidL2Book,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidL2Book {
    coin: String,
    levels: Vec<Vec<HyperliquidLevel>>,
    time: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidLevel {
    px: String,
    sz: String,
    n: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidTradesMsg {
    channel: String,
    data: Vec<HyperliquidTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidTrade {
    coin: String,
    side: String, // "A" for ask/sell, "B" for bid/buy
    px: String,
    sz: String,
    time: i64,
    hash: String,
    tid: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidCandleMsg {
    channel: String,
    data: HyperliquidCandle,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidCandle {
    t: i64,  // Start time
    #[serde(rename = "T")]
    #[serde(default)]
    end_time: Option<i64>,  // End time
    s: String,  // Symbol
    i: String,  // Interval
    o: String,  // Open
    c: String,  // Close
    h: String,  // High
    l: String,  // Low
    v: String,  // Volume
    #[serde(default)]
    n: Option<i32>,  // Number of trades
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_to_coin() {
        assert_eq!(HyperliquidWs::symbol_to_coin("BTC/USDC:USDC"), "BTC");
        assert_eq!(HyperliquidWs::symbol_to_coin("PURR/USDC"), "PURR/USDC");
    }

    #[test]
    fn test_coin_to_symbol() {
        assert_eq!(HyperliquidWs::coin_to_symbol("BTC", true), "BTC/USDC:USDC");
        assert_eq!(HyperliquidWs::coin_to_symbol("PURR/USDC", false), "PURR/USDC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(HyperliquidWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(HyperliquidWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(HyperliquidWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_l2book_message() {
        let json = r#"{
            "channel": "l2Book",
            "data": {
                "coin": "BTC",
                "time": "1710131872708",
                "levels": [
                    [
                        {"px": "68674.0", "sz": "0.97139", "n": 4}
                    ],
                    [
                        {"px": "68675.0", "sz": "0.04396", "n": 1}
                    ]
                ]
            }
        }"#;

        let msg: HyperliquidL2BookMsg = serde_json::from_str(json).unwrap();
        let result = HyperliquidWs::parse_order_book_message(&msg.data);

        assert!(result.is_some());
        if let Some(WsMessage::OrderBook(event)) = result {
            assert_eq!(event.symbol, "BTC/USDC:USDC");
            assert!(!event.order_book.bids.is_empty());
            assert!(!event.order_book.asks.is_empty());
        }
    }

    #[test]
    fn test_parse_trades_message() {
        let trades = vec![
            HyperliquidTrade {
                coin: "BTC".to_string(),
                side: "A".to_string(),
                px: "68517.0".to_string(),
                sz: "0.005".to_string(),
                time: 1710125266669,
                hash: "0xabc123".to_string(),
                tid: 981894269203506,
            }
        ];

        let result = HyperliquidWs::parse_trades_message(&trades);

        assert!(result.is_some());
        if let Some(WsMessage::Trade(event)) = result {
            assert_eq!(event.symbol, "BTC/USDC:USDC");
            assert_eq!(event.trades.len(), 1);
            assert_eq!(event.trades[0].side, Some("sell".to_string()));
        }
    }
}
