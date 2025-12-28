//! Gate.io WebSocket Implementation
//!
//! Gate.io 실시간 데이터 스트리밍

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

const WS_PUBLIC_URL: &str = "wss://api.gateio.ws/ws/v4/";

/// Gate.io WebSocket 클라이언트
pub struct GateWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl GateWs {
    /// 새 Gate.io WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Gate 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Gate 심볼을 통합 심볼로 변환 (BTC_USDT -> BTC/USDT)
    fn to_unified_symbol(gate_symbol: &str) -> String {
        gate_symbol.replace("_", "/")
    }

    /// Timeframe을 Gate 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Second1 => "10s",
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Week1 => "7d",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &GateTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.time.map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low_24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.highest_bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.lowest_ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change_percentage.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_percentage.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.base_volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &GateOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.t
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.u,
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
    fn parse_trade(data: &GateTradeData) -> WsTradeEvent {
        let symbol = Self::to_unified_symbol(&data.currency_pair);
        let timestamp = data.create_time.map(|t| (t * 1000.0) as i64)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.to_string(),
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
    fn parse_candle(data: &GateCandleData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let timestamp = data.t.parse::<i64>().ok()? * 1000;
        let ohlcv = OHLCV::new(
            timestamp,
            data.o.parse().ok()?,
            data.h.parse().ok()?,
            data.l.parse().ok()?,
            data.c.parse().ok()?,
            data.v.parse().ok()?,
        );

        let symbol = Self::to_unified_symbol(&data.n);

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<GateWsResponse>(msg) {
            // Event handling
            if let Some(event) = &response.event {
                if event == "subscribe" {
                    return Some(WsMessage::Subscribed {
                        channel: response.channel.clone().unwrap_or_default(),
                        symbol: None,
                    });
                }
            }

            // Data handling
            if let (Some(channel), Some(result)) = (&response.channel, &response.result) {
                // Ticker
                if channel == "spot.tickers" {
                    if let Ok(ticker_data) = serde_json::from_value::<GateTickerData>(result.clone()) {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                    }
                }

                // OrderBook
                if channel == "spot.order_book_update" {
                    if let Ok(ob_data) = serde_json::from_value::<GateOrderBookData>(result.clone()) {
                        let symbol = ob_data.s.as_ref()
                            .map(|s| Self::to_unified_symbol(s))
                            .unwrap_or_default();
                        let is_snapshot = response.event.as_deref() == Some("all");
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &symbol, is_snapshot)));
                    }
                }

                // Trades
                if channel == "spot.trades" {
                    if let Ok(trade_data) = serde_json::from_value::<GateTradeData>(result.clone()) {
                        return Some(WsMessage::Trade(Self::parse_trade(&trade_data)));
                    }
                }

                // Candles
                if channel == "spot.candlesticks" {
                    if let Ok(candle_data) = serde_json::from_value::<GateCandleData>(result.clone()) {
                        // Extract timeframe from candle name (e.g., "1m_BTC_USDT")
                        let parts: Vec<&str> = candle_data.n.split('_').collect();
                        let timeframe = if !parts.is_empty() {
                            match parts[0] {
                                "10s" => Timeframe::Second1,
                                "1m" => Timeframe::Minute1,
                                "3m" => Timeframe::Minute3,
                                "5m" => Timeframe::Minute5,
                                "15m" => Timeframe::Minute15,
                                "30m" => Timeframe::Minute30,
                                "1h" => Timeframe::Hour1,
                                "2h" => Timeframe::Hour2,
                                "4h" => Timeframe::Hour4,
                                "6h" => Timeframe::Hour6,
                                "8h" => Timeframe::Hour8,
                                "12h" => Timeframe::Hour12,
                                "1d" => Timeframe::Day1,
                                "7d" => Timeframe::Week1,
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

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(&mut self, channel: &str, payload: Vec<String>, channel_name: &str, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
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
        let timestamp = Utc::now().timestamp();
        let subscribe_msg = serde_json::json!({
            "time": timestamp,
            "channel": channel,
            "event": "subscribe",
            "payload": payload
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel_name, symbol.unwrap_or(""));
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

impl Default for GateWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for GateWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        client.subscribe_stream("spot.tickers", vec![market_id], "ticker", Some(symbol)).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client.subscribe_stream("spot.tickers", market_ids, "tickers", None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        // Gate uses "100ms" for real-time updates
        client.subscribe_stream("spot.order_book_update", vec![market_id, "100ms".to_string()], "orderBook", Some(symbol)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        client.subscribe_stream("spot.trades", vec![market_id], "trades", Some(symbol)).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscription = format!("{interval}_{market_id}");
        client.subscribe_stream("spot.candlesticks", vec![subscription], "ohlcv", Some(symbol)).await
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

// === Gate.io WebSocket Types ===

#[derive(Debug, Deserialize)]
struct GateWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateTickerData {
    currency_pair: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    lowest_ask: Option<String>,
    #[serde(default)]
    highest_bid: Option<String>,
    #[serde(default)]
    change_percentage: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(default)]
    time: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateOrderBookData {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    u: Option<i64>,
    #[serde(default)]
    s: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateTradeData {
    id: i64,
    currency_pair: String,
    price: String,
    amount: String,
    side: String,
    #[serde(default)]
    create_time: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GateCandleData {
    t: String,  // timestamp
    o: String,  // open
    h: String,  // high
    l: String,  // low
    c: String,  // close
    v: String,  // volume
    n: String,  // name (interval_symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(GateWs::format_symbol("BTC/USDT"), "BTC_USDT");
        assert_eq!(GateWs::format_symbol("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(GateWs::to_unified_symbol("BTC_USDT"), "BTC/USDT");
        assert_eq!(GateWs::to_unified_symbol("ETH_BTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(GateWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(GateWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(GateWs::format_interval(Timeframe::Day1), "1d");
    }
}
