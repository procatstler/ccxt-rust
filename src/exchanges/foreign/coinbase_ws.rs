//! Coinbase Advanced Trade WebSocket Implementation
//!
//! Coinbase 실시간 데이터 스트리밍

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

const WS_PUBLIC_URL: &str = "wss://advanced-trade-ws.coinbase.com";

/// Coinbase WebSocket 클라이언트
pub struct CoinbaseWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl CoinbaseWs {
    /// 새 Coinbase WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 심볼을 Coinbase 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Coinbase 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(coinbase_symbol: &str) -> String {
        coinbase_symbol.replace("-", "/")
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinbaseTickerEvent) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
            high: data.high_24_h,
            low: data.low_24_h,
            bid: data.best_bid,
            bid_volume: data.best_bid_quantity,
            ask: data.best_ask,
            ask_volume: data.best_ask_quantity,
            vwap: None,
            open: None,
            close: data.price,
            last: data.price,
            previous_close: None,
            change: None,
            percentage: data.price_percent_chg_24_h,
            average: None,
            base_volume: data.volume_24_h,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book_snapshot(data: &CoinbaseLevel2Event) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.updates.iter()
            .filter(|u| u.side.as_deref() == Some("bid"))
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.price_level?,
                    amount: u.new_quantity?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.updates.iter()
            .filter(|u| u.side.as_deref() == Some("offer"))
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.price_level?,
                    amount: u.new_quantity?,
                })
            })
            .collect();

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

    /// 체결 메시지 파싱
    fn parse_trades(data: &CoinbaseMarketTradesEvent) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);

        let trades: Vec<Trade> = data.trades.iter().map(|t| {
            let timestamp = t.time
                .as_ref()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            Trade {
                id: t.trade_id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: t.time.clone(),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: None,
                price: t.price.unwrap_or_default(),
                amount: t.size.unwrap_or_default(),
                cost: t.price.and_then(|p| t.size.map(|s| p * s)),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
            }
        }).collect();

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse the JSON message
        if let Ok(response) = serde_json::from_str::<CoinbaseWsResponse>(msg) {
            match response.channel.as_deref() {
                Some("subscriptions") => {
                    // Subscription confirmation
                    return Some(WsMessage::Subscribed {
                        channel: "subscriptions".to_string(),
                        symbol: None,
                    });
                }
                Some("ticker") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(tickers) = event.tickers {
                                if let Some(ticker_data) = tickers.first() {
                                    return Some(WsMessage::Ticker(Self::parse_ticker(ticker_data)));
                                }
                            }
                        }
                    }
                }
                Some("l2_data") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if event.event_type.as_deref() == Some("snapshot") ||
                               event.event_type.as_deref() == Some("update") {
                                let is_snapshot = event.event_type.as_deref() == Some("snapshot");
                                if let Some(product_id) = &event.product_id {
                                    let unified_symbol = Self::to_unified_symbol(product_id);
                                    let timestamp = Utc::now().timestamp_millis();

                                    let bids: Vec<OrderBookEntry> = event.updates.as_ref()
                                        .map(|updates| updates.iter()
                                            .filter(|u| u.side.as_deref() == Some("bid"))
                                            .filter_map(|u| {
                                                Some(OrderBookEntry {
                                                    price: u.price_level?,
                                                    amount: u.new_quantity?,
                                                })
                                            })
                                            .collect())
                                        .unwrap_or_default();

                                    let asks: Vec<OrderBookEntry> = event.updates.as_ref()
                                        .map(|updates| updates.iter()
                                            .filter(|u| u.side.as_deref() == Some("offer"))
                                            .filter_map(|u| {
                                                Some(OrderBookEntry {
                                                    price: u.price_level?,
                                                    amount: u.new_quantity?,
                                                })
                                            })
                                            .collect())
                                        .unwrap_or_default();

                                    let order_book = OrderBook {
                                        symbol: unified_symbol.clone(),
                                        timestamp: Some(timestamp),
                                        datetime: Some(Utc::now().to_rfc3339()),
                                        nonce: None,
                                        bids,
                                        asks,
                                    };

                                    return Some(WsMessage::OrderBook(WsOrderBookEvent {
                                        symbol: unified_symbol,
                                        order_book,
                                        is_snapshot,
                                    }));
                                }
                            }
                        }
                    }
                }
                Some("market_trades") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(trades) = &event.trades {
                                if !trades.is_empty() {
                                    let product_id = event.product_id.clone()
                                        .or_else(|| trades.first().and_then(|t| t.product_id.clone()))
                                        .unwrap_or_default();

                                    let unified_symbol = Self::to_unified_symbol(&product_id);

                                    let parsed_trades: Vec<Trade> = trades.iter().map(|t| {
                                        let timestamp = t.time
                                            .as_ref()
                                            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                                            .map(|dt| dt.timestamp_millis())
                                            .unwrap_or_else(|| Utc::now().timestamp_millis());

                                        Trade {
                                            id: t.trade_id.clone().unwrap_or_default(),
                                            order: None,
                                            timestamp: Some(timestamp),
                                            datetime: t.time.clone(),
                                            symbol: unified_symbol.clone(),
                                            trade_type: None,
                                            side: t.side.clone(),
                                            taker_or_maker: None,
                                            price: t.price.unwrap_or_default(),
                                            amount: t.size.unwrap_or_default(),
                                            cost: t.price.and_then(|p| t.size.map(|s| p * s)),
                                            fee: None,
                                            fees: Vec::new(),
                                            info: serde_json::to_value(t).unwrap_or_default(),
                                        }
                                    }).collect();

                                    return Some(WsMessage::Trade(WsTradeEvent {
                                        symbol: unified_symbol,
                                        trades: parsed_trades,
                                    }));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        product_ids: Vec<String>,
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
            "type": "subscribe",
            "product_ids": product_ids,
            "channel": channel
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, product_ids.join(","));
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

impl Default for CoinbaseWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinbaseWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], "ticker").await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(coinbase_symbols, "ticker").await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], "level2").await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], "market_trades").await
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coinbase doesn't support OHLCV via WebSocket
        // Return an error or use ticker to simulate
        Err(crate::errors::CcxtError::NotSupported {
            feature: "Coinbase WebSocket does not support OHLCV streaming".into(),
        })
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

// === Coinbase WebSocket Types ===

#[derive(Debug, Deserialize)]
struct CoinbaseWsResponse {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    events: Option<Vec<CoinbaseWsEvent>>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    sequence_num: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoinbaseWsEvent {
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    tickers: Option<Vec<CoinbaseTickerEvent>>,
    #[serde(default)]
    updates: Option<Vec<CoinbaseLevel2Update>>,
    #[serde(default)]
    trades: Option<Vec<CoinbaseTradeEvent>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseTickerEvent {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    volume_24_h: Option<Decimal>,
    #[serde(default)]
    low_24_h: Option<Decimal>,
    #[serde(default)]
    high_24_h: Option<Decimal>,
    #[serde(default)]
    low_52_w: Option<Decimal>,
    #[serde(default)]
    high_52_w: Option<Decimal>,
    #[serde(default)]
    price_percent_chg_24_h: Option<Decimal>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_bid_quantity: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    best_ask_quantity: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseLevel2Event {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    updates: Vec<CoinbaseLevel2Update>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseLevel2Update {
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price_level: Option<Decimal>,
    #[serde(default)]
    new_quantity: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseMarketTradesEvent {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    trades: Vec<CoinbaseTradeEvent>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseTradeEvent {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinbaseWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(CoinbaseWs::format_symbol("ETH/USD"), "ETH-USD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinbaseWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(CoinbaseWs::to_unified_symbol("ETH-USD"), "ETH/USD");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = CoinbaseTickerEvent {
            product_id: "BTC-USD".to_string(),
            price: Some(Decimal::from(50000)),
            volume_24_h: Some(Decimal::from(1000)),
            high_24_h: Some(Decimal::from(51000)),
            low_24_h: Some(Decimal::from(49000)),
            best_bid: Some(Decimal::from(49990)),
            best_ask: Some(Decimal::from(50010)),
            ..Default::default()
        };

        let result = CoinbaseWs::parse_ticker(&ticker_data);
        assert_eq!(result.symbol, "BTC/USD");
        assert_eq!(result.ticker.last, Some(Decimal::from(50000)));
    }
}
