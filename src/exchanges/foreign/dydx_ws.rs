//! dYdX WebSocket Implementation
//!
//! dYdX v4 real-time data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent, OHLCV,
};

const WS_PUBLIC_URL: &str = "wss://indexer.dydx.trade/v4/ws";

/// dYdX WebSocket client
pub struct DydxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl DydxWs {
    /// Create new dYdX WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Convert symbol to dYdX WebSocket format (BTC/USD -> BTC-USD)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert dYdX symbol to unified format (BTC-USD -> BTC/USD)
    fn to_unified_symbol(dydx_symbol: &str) -> String {
        dydx_symbol.replace("-", "/")
    }

    /// Parse ticker message from v4_markets channel
    fn parse_ticker(data: &DydxMarketData) -> Option<WsTickerEvent> {
        let symbol = Self::to_unified_symbol(data.ticker.as_ref()?);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: None,
            ask: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: None,
            index_price: data.index_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            mark_price: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTickerEvent { symbol, ticker })
    }

    /// Parse order book message from v4_orderbook channel
    fn parse_order_book(data: &DydxOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<DydxOrderBookLevel>| -> Vec<OrderBookEntry> {
            entries.iter().filter_map(|e| {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&e.price).ok()?,
                    amount: Decimal::from_str(&e.size).ok()?,
                })
            }).collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
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
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// Parse trade message from v4_trades channel
    fn parse_trades(data: &DydxTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let trades: Vec<Trade> = data.trades.iter().filter_map(|t| {
            let timestamp = t.created_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let price = Decimal::from_str(&t.price).ok()?;
            let amount = Decimal::from_str(&t.size).ok()?;

            Some(Trade {
                id: t.id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
            })
        }).collect();

        WsTradeEvent {
            symbol: unified_symbol,
            trades,
        }
    }

    /// Parse OHLCV message from v4_candles channel
    fn parse_candles(data: &DydxCandleData, symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let candle = data.candles.first()?;
        let timestamp = candle.started_at.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())?;

        let ohlcv = OHLCV {
            timestamp,
            open: Decimal::from_str(&candle.open).ok()?,
            high: Decimal::from_str(&candle.high).ok()?,
            low: Decimal::from_str(&candle.low).ok()?,
            close: Decimal::from_str(&candle.close).ok()?,
            volume: Decimal::from_str(&candle.base_token_volume).ok()?,
        };

        Some(WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        })
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: DydxWsMessage = serde_json::from_str(msg).ok()?;

        match json.msg_type.as_str() {
            "connected" => Some(WsMessage::Connected),
            "subscribed" => {
                Some(WsMessage::Subscribed {
                    channel: json.channel?,
                    symbol: json.id.clone(),
                })
            }
            "channel_data" => {
                let channel = json.channel.as_ref()?;
                let contents = json.contents.as_ref()?;

                if channel == "v4_markets" {
                    if let Ok(market_data) = serde_json::from_value::<DydxMarketData>(contents.clone()) {
                        return Self::parse_ticker(&market_data).map(WsMessage::Ticker);
                    }
                } else if channel == "v4_orderbook" {
                    if let Ok(ob_data) = serde_json::from_value::<DydxOrderBookData>(contents.clone()) {
                        let symbol = json.id.as_ref()?;
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, symbol)));
                    }
                } else if channel == "v4_trades" {
                    if let Ok(trade_data) = serde_json::from_value::<DydxTradeData>(contents.clone()) {
                        let symbol = json.id.as_ref()?;
                        return Some(WsMessage::Trade(Self::parse_trades(&trade_data, symbol)));
                    }
                } else if channel == "v4_candles" {
                    if let Ok(candle_data) = serde_json::from_value::<DydxCandleData>(contents.clone()) {
                        // Extract symbol and timeframe from id (format: "BTC-USD/1MIN")
                        let id_parts: Vec<&str> = json.id.as_ref()?.split('/').collect();
                        if id_parts.len() == 2 {
                            let symbol = id_parts[0];
                            let timeframe = match id_parts[1] {
                                "1MIN" => Timeframe::Minute1,
                                "5MINS" => Timeframe::Minute5,
                                "15MINS" => Timeframe::Minute15,
                                "30MINS" => Timeframe::Minute30,
                                "1HOUR" => Timeframe::Hour1,
                                "4HOURS" => Timeframe::Hour4,
                                "1DAY" => Timeframe::Day1,
                                _ => return None,
                            };
                            return Self::parse_candles(&candle_data, symbol, timeframe).map(WsMessage::Ohlcv);
                        }
                    }
                }

                None
            }
            _ => None,
        }
    }

    /// Subscribe to a channel and return event stream
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        id: &str,
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

        // Build subscription message
        let subscribe_msg = DydxSubscribeMessage {
            msg_type: "subscribe".to_string(),
            channel: channel.to_string(),
            id: id.to_string(),
        };

        ws_client.send(&serde_json::to_string(&subscribe_msg).unwrap())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{}:{}", channel, id);
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Event processing task
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

#[async_trait]
impl WsExchange for DydxWs {
    /// Subscribe to ticker updates
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = DydxWs::new();
        let formatted_symbol = Self::format_symbol(symbol);
        ws.subscribe_stream("v4_markets", &formatted_symbol).await
    }

    /// Subscribe to order book updates
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = DydxWs::new();
        let formatted_symbol = Self::format_symbol(symbol);
        ws.subscribe_stream("v4_orderbook", &formatted_symbol).await
    }

    /// Subscribe to trade updates
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = DydxWs::new();
        let formatted_symbol = Self::format_symbol(symbol);
        ws.subscribe_stream("v4_trades", &formatted_symbol).await
    }

    /// Subscribe to OHLCV updates
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = DydxWs::new();
        let formatted_symbol = Self::format_symbol(symbol);

        // Convert timeframe to dYdX format
        let tf_str = match timeframe {
            Timeframe::Minute1 => "1MIN",
            Timeframe::Minute5 => "5MINS",
            Timeframe::Minute15 => "15MINS",
            Timeframe::Minute30 => "30MINS",
            Timeframe::Hour1 => "1HOUR",
            Timeframe::Hour4 => "4HOURS",
            Timeframe::Day1 => "1DAY",
            _ => return Err(crate::errors::CcxtError::NotSupported {
                feature: format!("Timeframe {:?}", timeframe),
            }),
        };

        let id = format!("{}/{}", formatted_symbol, tf_str);
        ws.subscribe_stream("v4_candles", &id).await
    }

    /// Connect to WebSocket
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_PUBLIC_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });

            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    /// Close WebSocket connection
    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }

    /// Check if WebSocket is connected
    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

impl Default for DydxWs {
    fn default() -> Self {
        Self::new()
    }
}

// ===== dYdX WebSocket Message Structures =====

#[derive(Debug, Deserialize)]
struct DydxWsMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    contents: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct DydxSubscribeMessage {
    #[serde(rename = "type")]
    msg_type: String,
    channel: String,
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DydxMarketData {
    #[serde(default)]
    ticker: Option<String>,
    #[serde(default, rename = "oraclePrice")]
    oracle_price: Option<String>,
    #[serde(default, rename = "indexPrice")]
    index_price: Option<String>,
    #[serde(default, rename = "priceChange24H")]
    price_change_24h: Option<String>,
    #[serde(default, rename = "high24H")]
    high_24h: Option<String>,
    #[serde(default, rename = "low24H")]
    low_24h: Option<String>,
    #[serde(default, rename = "volume24H")]
    volume_24h: Option<String>,
    #[serde(default, rename = "trades24H")]
    trades_24h: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxOrderBookData {
    #[serde(default)]
    bids: Vec<DydxOrderBookLevel>,
    #[serde(default)]
    asks: Vec<DydxOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct DydxOrderBookLevel {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct DydxTradeData {
    #[serde(default)]
    trades: Vec<DydxTrade>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DydxTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    size: String,
    price: String,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxCandleData {
    #[serde(default)]
    candles: Vec<DydxCandle>,
}

#[derive(Debug, Deserialize)]
struct DydxCandle {
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    open: String,
    high: String,
    low: String,
    close: String,
    #[serde(rename = "baseTokenVolume")]
    base_token_volume: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(DydxWs::format_symbol("BTC/USD"), "BTC-USD");
        assert_eq!(DydxWs::format_symbol("ETH/USD"), "ETH-USD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DydxWs::to_unified_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(DydxWs::to_unified_symbol("ETH-USD"), "ETH/USD");
    }

    #[test]
    fn test_ws_client_creation() {
        let ws = DydxWs::new();
        assert!(ws.ws_client.is_none());
    }
}
