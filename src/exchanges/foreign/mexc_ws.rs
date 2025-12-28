//! MEXC WebSocket Implementation
//!
//! MEXC 실시간 데이터 스트리밍

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://wbs.mexc.com/ws";

/// MEXC WebSocket 클라이언트
pub struct MexcWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl MexcWs {
    /// 새 MEXC WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 MEXC 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "").to_uppercase()
    }

    /// MEXC 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(mexc_symbol: &str) -> String {
        let quote_currencies = ["USDT", "USDC", "BTC", "ETH", "BUSD", "TUSD"];
        let upper = mexc_symbol.to_uppercase();

        for quote in &quote_currencies {
            if upper.ends_with(quote) {
                let base = &upper[..upper.len() - quote.len()];
                return format!("{base}/{quote}");
            }
        }

        mexc_symbol.to_uppercase()
    }

    /// Timeframe을 MEXC 포맷으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "Min1",
            Timeframe::Minute5 => "Min5",
            Timeframe::Minute15 => "Min15",
            Timeframe::Minute30 => "Min30",
            Timeframe::Hour1 => "Hour1",
            Timeframe::Hour4 => "Hour4",
            Timeframe::Hour8 => "Hour8",
            Timeframe::Day1 => "Day1",
            Timeframe::Week1 => "Week1",
            Timeframe::Month1 => "Month1",
            _ => "Min1",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &MexcTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h.as_ref().and_then(|v| v.parse().ok()),
            low: data.l.as_ref().and_then(|v| v.parse().ok()),
            bid: data.b.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.a.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.o.as_ref().and_then(|v| v.parse().ok()),
            close: data.c.as_ref().and_then(|v| v.parse().ok()),
            last: data.c.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.r.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.q.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &MexcDepthData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(|b| {
            Some(OrderBookEntry {
                price: b.p.as_ref()?.parse().ok()?,
                amount: b.v.as_ref()?.parse().ok()?,
            })
        }).collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(|a| {
            Some(OrderBookEntry {
                price: a.p.as_ref()?.parse().ok()?,
                amount: a.v.as_ref()?.parse().ok()?,
            })
        }).collect();

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.r.map(|r| r as i64),
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
    fn parse_trade(data: &MexcTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = data.side.as_ref().map(|s| {
            match s.as_str() {
                "1" => "buy".to_string(),
                "2" => "sell".to_string(),
                _ => s.to_lowercase(),
            }
        });

        let trades = vec![Trade {
            id: data.t.map(|t| t.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price: data.p.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            amount: data.v.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            cost: data.p.as_ref().and_then(|p| {
                data.v.as_ref().and_then(|v| {
                    let price: Decimal = p.parse().ok()?;
                    let amount: Decimal = v.parse().ok()?;
                    Some(price * amount)
                })
            }),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }];

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// K선 메시지 파싱
    fn parse_kline(data: &MexcKlineData, symbol: &str, timeframe: Timeframe) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: data.o.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            high: data.h.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            low: data.l.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            close: data.c.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
            volume: data.v.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Option<WsMessage> {
        // 구독 확인
        if let Ok(response) = serde_json::from_str::<MexcWsResponse>(msg) {
            if response.code.is_some() {
                // Subscription response
                if let Some(channel) = response.c {
                    return Some(WsMessage::Subscribed {
                        channel,
                        symbol: None,
                    });
                }
                return None;
            }

            // 데이터 메시지
            if let Some(channel) = &response.c {
                // Parse channel to extract symbol
                // Format: spot@public.ticker.v3.api@BTCUSDT
                let parts: Vec<&str> = channel.split('@').collect();
                let symbol = parts.last().unwrap_or(&"");

                if channel.contains("ticker") {
                    if let Some(d) = response.d {
                        if let Ok(ticker_data) = serde_json::from_value::<MexcTickerData>(d) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    }
                } else if channel.contains("limit.depth") || channel.contains("depth") {
                    if let Some(d) = response.d {
                        if let Ok(depth_data) = serde_json::from_value::<MexcDepthData>(d) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&depth_data, symbol)));
                        }
                    }
                } else if channel.contains("deals") || channel.contains("trade") {
                    if let Some(d) = response.d {
                        if let Ok(trade_data) = serde_json::from_value::<MexcDealsWrapper>(d) {
                            if let Some(first_trade) = trade_data.deals.first() {
                                return Some(WsMessage::Trade(Self::parse_trade(first_trade, symbol)));
                            }
                        }
                    }
                } else if channel.contains("kline") {
                    if let Some(d) = response.d {
                        if let Ok(kline_data) = serde_json::from_value::<MexcKlineWrapper>(d) {
                            let tf = timeframe.unwrap_or(Timeframe::Minute1);
                            return Some(WsMessage::Ohlcv(Self::parse_kline(&kline_data.k, symbol, tf)));
                        }
                    }
                }
            }
        }

        // Ping response
        if msg.contains("\"msg\":\"PONG\"") {
            return None;
        }

        None
    }

    /// 구독 메시지 생성
    fn create_subscription_msg(channel: &str) -> String {
        serde_json::json!({
            "method": "SUBSCRIPTION",
            "params": [channel]
        }).to_string()
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        channel_type: &str,
        timeframe: Option<Timeframe>,
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
        let subscribe_msg = Self::create_subscription_msg(channel);
        ws_client.send(&subscribe_msg)?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = channel.to_string();
            self.subscriptions.write().await.insert(key, channel_type.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let tf = timeframe;
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
                        if let Some(ws_msg) = Self::process_message(&msg, tf) {
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

impl Default for MexcWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for MexcWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let channel = format!("spot@public.ticker.v3.api@{mexc_symbol}");
        client.subscribe_stream(&channel, "ticker", None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // MEXC doesn't support batch ticker subscription, subscribe to first symbol
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
        let mexc_symbol = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(5);
        let channel = format!("spot@public.limit.depth.v3.api@{mexc_symbol}@{depth}");
        client.subscribe_stream(&channel, "orderBook", None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let channel = format!("spot@public.deals.v3.api@{mexc_symbol}");
        client.subscribe_stream(&channel, "trades", None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let mexc_symbol = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let channel = format!("spot@public.kline.v3.api@{mexc_symbol}@{interval}");
        client.subscribe_stream(&channel, "ohlcv", Some(timeframe)).await
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

// === MEXC WebSocket Types ===

#[derive(Debug, Deserialize)]
struct MexcWsResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    d: Option<serde_json::Value>,
    #[serde(default)]
    t: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcTickerData {
    #[serde(default)]
    o: Option<String>,  // open
    #[serde(default)]
    c: Option<String>,  // close
    #[serde(default)]
    h: Option<String>,  // high
    #[serde(default)]
    l: Option<String>,  // low
    #[serde(default)]
    v: Option<String>,  // volume
    #[serde(default)]
    q: Option<String>,  // quote volume
    #[serde(default)]
    r: Option<String>,  // price change rate
    #[serde(default)]
    b: Option<String>,  // bid
    #[serde(default)]
    a: Option<String>,  // ask
    #[serde(default)]
    t: Option<i64>,     // timestamp
}

#[derive(Debug, Default, Deserialize)]
struct MexcDepthData {
    #[serde(default)]
    bids: Vec<MexcDepthEntry>,
    #[serde(default)]
    asks: Vec<MexcDepthEntry>,
    #[serde(default)]
    r: Option<u64>,  // version
}

#[derive(Debug, Default, Deserialize)]
struct MexcDepthEntry {
    #[serde(default)]
    p: Option<String>,  // price
    #[serde(default)]
    v: Option<String>,  // volume
}

#[derive(Debug, Default, Deserialize)]
struct MexcDealsWrapper {
    #[serde(default)]
    deals: Vec<MexcTradeData>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcTradeData {
    #[serde(default)]
    p: Option<String>,  // price
    #[serde(default)]
    v: Option<String>,  // volume
    #[serde(default, rename = "S")]
    side: Option<String>,  // side: 1=buy, 2=sell
    #[serde(default)]
    t: Option<i64>,     // timestamp
}

#[derive(Debug, Default, Deserialize)]
struct MexcKlineWrapper {
    #[serde(default)]
    k: MexcKlineData,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct MexcKlineData {
    #[serde(default)]
    t: Option<i64>,     // timestamp
    #[serde(default)]
    o: Option<String>,  // open
    #[serde(default)]
    c: Option<String>,  // close
    #[serde(default)]
    h: Option<String>,  // high
    #[serde(default)]
    l: Option<String>,  // low
    #[serde(default)]
    v: Option<String>,  // volume
    #[serde(default)]
    q: Option<String>,  // quote volume
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(MexcWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(MexcWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(MexcWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(MexcWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(MexcWs::format_interval(Timeframe::Minute1), "Min1");
        assert_eq!(MexcWs::format_interval(Timeframe::Hour1), "Hour1");
        assert_eq!(MexcWs::format_interval(Timeframe::Day1), "Day1");
    }
}
