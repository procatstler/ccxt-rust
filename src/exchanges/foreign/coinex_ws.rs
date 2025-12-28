//! CoinEx WebSocket Implementation
//!
//! CoinEx 실시간 데이터 스트리밍

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

const WS_PUBLIC_URL: &str = "wss://socket.coinex.com/v2/spot";

/// CoinEx WebSocket 클라이언트
pub struct CoinexWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl CoinexWs {
    /// 새 CoinEx WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 CoinEx 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// CoinEx 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(coinex_symbol: &str) -> String {
        for quote in &["USDT", "USDC", "BTC", "ETH"] {
            if let Some(base) = coinex_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        coinex_symbol.to_string()
    }

    /// Timeframe을 CoinEx 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> i32 {
        match timeframe {
            Timeframe::Minute1 => 60,
            Timeframe::Minute5 => 300,
            Timeframe::Minute15 => 900,
            Timeframe::Minute30 => 1800,
            Timeframe::Hour1 => 3600,
            Timeframe::Hour4 => 14400,
            Timeframe::Day1 => 86400,
            Timeframe::Week1 => 604800,
            _ => 60,
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinexTickerData, symbol: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.buy.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.buy_amount.as_ref().and_then(|v| v.parse().ok()),
            ask: data.sell.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.sell_amount.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last.as_ref().and_then(|v| v.parse().ok()),
            last: data.last.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &CoinexOrderBookData, symbol: &str) -> WsOrderBookEvent {
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

        let timestamp = Utc::now().timestamp_millis();
        let unified_symbol = Self::to_unified_symbol(symbol);

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
    fn parse_trade(data: &CoinexTradeData, symbol: &str) -> WsTradeEvent {
        let timestamp = data.date_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
        let unified_symbol = Self::to_unified_symbol(symbol);
        let price = data.price.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();
        let amount = data.amount.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let trades = vec![Trade {
            id: data.id.map(|id| id.to_string()).unwrap_or_default(),
            info: serde_json::Value::Null,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            order: None,
            trade_type: None,
            side: data.trade_type.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
        }];

        WsTradeEvent {
            symbol: unified_symbol,
            trades,
        }
    }

    /// 캔들 메시지 파싱
    fn parse_candle(data: &[String], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let unified_symbol = Self::to_unified_symbol(symbol);

        let ohlcv = OHLCV {
            timestamp: data[0].parse().ok()?,
            open: data[1].parse().ok()?,
            close: data[2].parse().ok()?,
            high: data[3].parse().ok()?,
            low: data[4].parse().ok()?,
            volume: data[5].parse().ok()?,
        };

        Some(WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        let response: CoinexWsResponse = serde_json::from_str(msg).ok()?;

        let method = response.method.as_deref()?;
        let symbol = subscribed_symbol.unwrap_or("");

        match method {
            "state.update" => {
                if let Some(params) = response.params {
                    if let Some(first) = params.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<CoinexTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data, symbol)));
                        }
                    }
                }
            }
            "depth.update" => {
                if let Some(params) = response.params {
                    if params.len() >= 2 {
                        if let Ok(book_data) = serde_json::from_value::<CoinexOrderBookData>(params[1].clone()) {
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&book_data, symbol)));
                        }
                    }
                }
            }
            "deals.update" => {
                if let Some(params) = response.params {
                    if params.len() >= 2 {
                        if let Some(trades_arr) = params[1].as_array() {
                            if let Some(first) = trades_arr.first() {
                                if let Ok(trade_data) = serde_json::from_value::<CoinexTradeData>(first.clone()) {
                                    return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                                }
                            }
                        }
                    }
                }
            }
            "kline.update" => {
                if let Some(params) = response.params {
                    if let Some(first) = params.first() {
                        if let Ok(kline_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                            if let Some(event) = Self::parse_candle(&kline_arr, symbol, timeframe) {
                                return Some(WsMessage::Ohlcv(event));
                            }
                        }
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
        symbol: Option<&str>,
        timeframe: Option<Timeframe>
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_PUBLIC_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 15,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, symbol.unwrap_or(""));
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let subscribed_symbol = symbol.map(|s| s.to_string());
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
                        if let Some(ws_msg) = Self::process_message(&msg, subscribed_symbol.as_deref(), timeframe) {
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

impl Default for CoinexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "method": "state.subscribe",
            "params": [formatted],
            "id": 1
        });
        client.subscribe_stream(subscribe_msg, "ticker", Some(&formatted), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let params: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        let subscribe_msg = serde_json::json!({
            "method": "state.subscribe",
            "params": params,
            "id": 1
        });
        client.subscribe_stream(subscribe_msg, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let depth = limit.unwrap_or(20).to_string();
        let subscribe_msg = serde_json::json!({
            "method": "depth.subscribe",
            "params": [formatted, depth, "0"],
            "id": 2
        });
        client.subscribe_stream(subscribe_msg, "orderBook", Some(&formatted), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let subscribe_msg = serde_json::json!({
            "method": "deals.subscribe",
            "params": [formatted],
            "id": 3
        });
        client.subscribe_stream(subscribe_msg, "trade", Some(&formatted), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let formatted = Self::format_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let subscribe_msg = serde_json::json!({
            "method": "kline.subscribe",
            "params": [formatted, interval],
            "id": 4
        });
        client.subscribe_stream(subscribe_msg, "kline", Some(&formatted), Some(timeframe)).await
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

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct CoinexWsResponse {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexTickerData {
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
    #[serde(default)]
    buy: Option<String>,
    #[serde(default)]
    buy_amount: Option<String>,
    #[serde(default)]
    sell: Option<String>,
    #[serde(default)]
    sell_amount: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexTradeData {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    trade_type: Option<String>,
    #[serde(default)]
    date_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinexWs::format_symbol("BTC/USDT"), "BTCUSDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinexWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
    }
}
