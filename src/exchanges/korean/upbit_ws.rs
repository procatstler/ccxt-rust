//! Upbit WebSocket Implementation
//!
//! Upbit 실시간 데이터 스트리밍

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

const WS_PUBLIC_URL: &str = "wss://api.upbit.com/websocket/v1";

/// Upbit WebSocket 클라이언트
pub struct UpbitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl UpbitWs {
    /// 새 Upbit WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Upbit 형식으로 변환 (BTC/KRW -> KRW-BTC)
    fn format_symbol(symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            format!("{}-{}", parts[1], parts[0])
        } else {
            symbol.to_string()
        }
    }

    /// Upbit 심볼을 통합 심볼로 변환 (KRW-BTC -> BTC/KRW)
    fn to_unified_symbol(upbit_symbol: &str) -> String {
        let parts: Vec<&str> = upbit_symbol.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[1], parts[0])
        } else {
            upbit_symbol.to_string()
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &UpbitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price,
            low: data.low_price,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.opening_price,
            close: data.trade_price,
            last: data.trade_price,
            previous_close: data.prev_closing_price,
            change: data.signed_change_price,
            percentage: data.signed_change_rate.map(|r| r * Decimal::from(100)),
            average: None,
            base_volume: data.acc_trade_volume_24h,
            quote_volume: data.acc_trade_price_24h,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &UpbitOrderBookData) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(&data.code);

        let bids: Vec<OrderBookEntry> = data.orderbook_units.iter()
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.bid_price?,
                    amount: u.bid_size?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.orderbook_units.iter()
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.ask_price?,
                    amount: u.ask_size?,
                })
            })
            .collect();

        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

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

        WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,  // Upbit always sends full orderbook
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &UpbitTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.code);
        let timestamp = data.trade_timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.trade_price.unwrap_or_default();
        let amount: Decimal = data.trade_volume.unwrap_or_default();

        let side = match data.ask_bid.as_deref() {
            Some("ASK") => "sell",
            Some("BID") => "buy",
            _ => "buy",
        };

        let trades = vec![Trade {
            id: data.sequential_id.map(|id| id.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(side.to_string()),
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

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Try to parse as different message types
        if let Ok(ticker_data) = serde_json::from_str::<UpbitTickerData>(msg) {
            if ticker_data.msg_type.as_deref() == Some("ticker") {
                return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
            }
        }

        if let Ok(ob_data) = serde_json::from_str::<UpbitOrderBookData>(msg) {
            if ob_data.msg_type.as_deref() == Some("orderbook") {
                return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data)));
            }
        }

        if let Ok(trade_data) = serde_json::from_str::<UpbitTradeData>(msg) {
            if trade_data.msg_type.as_deref() == Some("trade") {
                return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        msg_type: &str,
        codes: Vec<String>,
        channel: &str,
        symbol: Option<&str>
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 120,  // Upbit requires ping every 2 minutes
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate unique ticket
        let ticket = uuid::Uuid::new_v4().to_string();

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!([
            {"ticket": ticket},
            {"type": msg_type, "codes": codes, "isOnlyRealtime": true}
        ]);
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

impl Default for UpbitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for UpbitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("ticker", codes, "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client.subscribe_stream("ticker", codes, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("orderbook", codes, "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let codes = vec![Self::format_symbol(symbol)];
        client.subscribe_stream("trade", codes, "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Upbit does not support OHLCV streaming via WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watch_ohlcv".to_string(),
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

// === Upbit WebSocket Types ===

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTickerData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    code: String,
    #[serde(default)]
    opening_price: Option<Decimal>,
    #[serde(default)]
    high_price: Option<Decimal>,
    #[serde(default)]
    low_price: Option<Decimal>,
    #[serde(default)]
    trade_price: Option<Decimal>,
    #[serde(default)]
    prev_closing_price: Option<Decimal>,
    #[serde(default)]
    signed_change_price: Option<Decimal>,
    #[serde(default)]
    signed_change_rate: Option<Decimal>,
    #[serde(default)]
    acc_trade_volume_24h: Option<Decimal>,
    #[serde(default)]
    acc_trade_price_24h: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderBookData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    code: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    orderbook_units: Vec<UpbitOrderBookUnit>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrderBookUnit {
    #[serde(default)]
    ask_price: Option<Decimal>,
    #[serde(default)]
    bid_price: Option<Decimal>,
    #[serde(default)]
    ask_size: Option<Decimal>,
    #[serde(default)]
    bid_size: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTradeData {
    #[serde(rename = "type", default)]
    msg_type: Option<String>,
    code: String,
    #[serde(default)]
    trade_price: Option<Decimal>,
    #[serde(default)]
    trade_volume: Option<Decimal>,
    #[serde(default)]
    ask_bid: Option<String>,
    #[serde(default)]
    trade_timestamp: Option<i64>,
    #[serde(default)]
    sequential_id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(UpbitWs::format_symbol("BTC/KRW"), "KRW-BTC");
        assert_eq!(UpbitWs::format_symbol("ETH/BTC"), "BTC-ETH");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(UpbitWs::to_unified_symbol("KRW-BTC"), "BTC/KRW");
        assert_eq!(UpbitWs::to_unified_symbol("BTC-ETH"), "ETH/BTC");
    }
}
