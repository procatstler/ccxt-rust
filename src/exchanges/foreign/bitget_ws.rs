//! Bitget WebSocket Implementation
//!
//! Bitget 실시간 데이터 스트리밍

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

const WS_PUBLIC_URL: &str = "wss://ws.bitget.com/spot/v1/stream";

/// Bitget WebSocket 클라이언트
pub struct BitgetWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BitgetWs {
    /// 새 Bitget WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Bitget 형식으로 변환 (BTC/USDT -> BTCUSDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// Bitget 심볼을 통합 심볼로 변환 (BTCUSDT -> BTC/USDT)
    fn to_unified_symbol(bitget_symbol: &str) -> String {
        // Common quote currencies
        for quote in &["USDT", "USDC", "BTC", "ETH"] {
            if let Some(base) = bitget_symbol.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }
        bitget_symbol.to_string()
    }

    /// Timeframe을 Bitget 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1H",
            Timeframe::Hour4 => "4H",
            Timeframe::Hour6 => "6H",
            Timeframe::Hour12 => "12H",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            _ => "1m",
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BitgetTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.inst_id);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high24h.as_ref().and_then(|v| v.parse().ok()),
            low: data.low24h.as_ref().and_then(|v| v.parse().ok()),
            bid: data.bid_pr.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bid_sz.as_ref().and_then(|v| v.parse().ok()),
            ask: data.ask_pr.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.ask_sz.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.open.as_ref().and_then(|v| v.parse().ok()),
            close: data.last_pr.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_pr.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.change_utc.as_ref().and_then(|v| {
                v.parse::<Decimal>().ok().map(|d| d * Decimal::from(100))
            }),
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
    fn parse_order_book(data: &BitgetOrderBookData, symbol: &str, is_snapshot: bool) -> WsOrderBookEvent {
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

        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let order_book = OrderBook {
            symbol: symbol.to_string(),
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
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BitgetTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = data.ts.as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

        let trades = vec![Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
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

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// OHLCV 메시지 파싱
    fn parse_candle(data: &[String], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        if data.len() < 6 {
            return None;
        }

        let timestamp = data[0].parse::<i64>().ok()?;
        let ohlcv = OHLCV::new(
            timestamp,
            data[1].parse().ok()?,
            data[2].parse().ok()?,
            data[3].parse().ok()?,
            data[4].parse().ok()?,
            data[5].parse().ok()?,
        );

        let unified_symbol = Self::to_unified_symbol(symbol);

        Some(WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str, subscribed_symbol: Option<&str>, subscribed_timeframe: Option<Timeframe>) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<BitgetWsResponse>(msg) {
            // Event handling (subscribe confirmation)
            if response.event.as_deref() == Some("subscribe") {
                return Some(WsMessage::Subscribed {
                    channel: response.arg.as_ref()
                        .and_then(|a| a.channel.clone())
                        .unwrap_or_default(),
                    symbol: response.arg.as_ref().and_then(|a| a.inst_id.clone()),
                });
            }

            // Data handling
            if let (Some(arg), Some(data_arr)) = (&response.arg, &response.data) {
                let channel = arg.channel.as_deref().unwrap_or("");
                let symbol = arg.inst_id.as_deref().unwrap_or("");

                // Ticker
                if channel == "ticker" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ticker_data) = serde_json::from_value::<BitgetTickerData>(first.clone()) {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                        }
                    }
                }

                // OrderBook
                if channel == "books" || channel == "books5" || channel == "books15" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(ob_data) = serde_json::from_value::<BitgetOrderBookData>(first.clone()) {
                            let unified_symbol = Self::to_unified_symbol(symbol);
                            let is_snapshot = response.action.as_deref() == Some("snapshot");
                            return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, &unified_symbol, is_snapshot)));
                        }
                    }
                }

                // Trade
                if channel == "trade" {
                    if let Some(first) = data_arr.first() {
                        if let Ok(trade_data) = serde_json::from_value::<BitgetTradeData>(first.clone()) {
                            return Some(WsMessage::Trade(Self::parse_trade(&trade_data, symbol)));
                        }
                    }
                }

                // Candle
                if channel.starts_with("candle") {
                    if let Some(first) = data_arr.first() {
                        if let Ok(candle_arr) = serde_json::from_value::<Vec<String>>(first.clone()) {
                            let timeframe = subscribed_timeframe.unwrap_or(Timeframe::Minute1);
                            let sym = subscribed_symbol.unwrap_or(symbol);
                            if let Some(event) = Self::parse_candle(&candle_arr, sym, timeframe) {
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
        args: Vec<serde_json::Value>,
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
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args
        });
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

impl Default for BitgetWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitgetWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": "ticker",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ticker", Some(symbol), None).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args: Vec<serde_json::Value> = symbols
            .iter()
            .map(|s| serde_json::json!({
                "instType": "sp",
                "channel": "ticker",
                "instId": Self::format_symbol(s)
            }))
            .collect();
        client.subscribe_stream(args, "tickers", None, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channel = match limit.unwrap_or(15) {
            1..=5 => "books5",
            6..=15 => "books15",
            _ => "books",
        };
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": channel,
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "orderBook", Some(symbol), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": "trade",
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "trades", Some(symbol), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let interval = Self::format_interval(timeframe);
        let args = vec![serde_json::json!({
            "instType": "sp",
            "channel": format!("candle{}", interval),
            "instId": Self::format_symbol(symbol)
        })];
        client.subscribe_stream(args, "ohlcv", Some(symbol), Some(timeframe)).await
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

// === Bitget WebSocket Types ===

#[derive(Debug, Deserialize)]
struct BitgetWsResponse {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    arg: Option<BitgetWsArg>,
    #[serde(default)]
    data: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BitgetWsArg {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    inst_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetTickerData {
    #[serde(default)]
    inst_id: String,
    #[serde(default)]
    last_pr: Option<String>,
    #[serde(default)]
    bid_pr: Option<String>,
    #[serde(default)]
    bid_sz: Option<String>,
    #[serde(default)]
    ask_pr: Option<String>,
    #[serde(default)]
    ask_sz: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high24h: Option<String>,
    #[serde(default)]
    low24h: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    change_utc: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitgetOrderBookData {
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    ts: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BitgetTradeData {
    trade_id: String,
    price: String,
    size: String,
    side: String,
    #[serde(default)]
    ts: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitgetWs::format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(BitgetWs::format_symbol("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitgetWs::to_unified_symbol("BTCUSDT"), "BTC/USDT");
        assert_eq!(BitgetWs::to_unified_symbol("ETHBTC"), "ETH/BTC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(BitgetWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BitgetWs::format_interval(Timeframe::Hour1), "1H");
        assert_eq!(BitgetWs::format_interval(Timeframe::Day1), "1D");
    }
}
