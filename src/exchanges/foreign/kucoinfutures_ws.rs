//! KuCoin Futures WebSocket Implementation
//!
//! KuCoin Futures 실시간 데이터 스트리밍
//! Note: KuCoin Futures requires a token from REST API before connecting to WebSocket

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV, TakerOrMaker,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
};

/// KuCoin Futures WebSocket 클라이언트
pub struct KucoinfuturesWs {
    config: ExchangeConfig,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    connect_id: String,
}

/// KuCoin Futures WebSocket token response
#[derive(Debug, Deserialize)]
struct KfWsTokenResponse {
    code: String,
    data: Option<KfWsTokenData>,
}

#[derive(Debug, Deserialize)]
struct KfWsTokenData {
    token: String,
    #[serde(rename = "instanceServers")]
    instance_servers: Vec<KfWsInstanceServer>,
}

#[derive(Debug, Deserialize)]
struct KfWsInstanceServer {
    endpoint: String,
    protocol: String,
    encrypt: bool,
    #[serde(rename = "pingInterval")]
    ping_interval: i64,
    #[serde(rename = "pingTimeout")]
    ping_timeout: i64,
}

/// KuCoin Futures ticker data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTickerData {
    symbol: String,
    sequence: Option<i64>,
    side: Option<String>,
    price: Option<String>,
    size: Option<i64>,
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(rename = "bestBidSize")]
    best_bid_size: Option<i64>,
    #[serde(rename = "bestBidPrice")]
    best_bid_price: Option<String>,
    #[serde(rename = "bestAskPrice")]
    best_ask_price: Option<String>,
    #[serde(rename = "bestAskSize")]
    best_ask_size: Option<i64>,
    ts: Option<i64>,
}

/// KuCoin Futures order book data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderBookData {
    sequence: Option<i64>,
    change: Option<String>,
    timestamp: Option<i64>,
}

/// KuCoin Futures order book snapshot
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOrderBookSnapshot {
    sequence: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    ts: Option<i64>,
}

/// KuCoin Futures trade data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsTradeData {
    sequence: Option<i64>,
    #[serde(rename = "tradeId")]
    trade_id: String,
    #[serde(rename = "makerOrderId")]
    maker_order_id: Option<String>,
    #[serde(rename = "takerOrderId")]
    taker_order_id: Option<String>,
    side: String,
    price: String,
    size: String,
    ts: i64,
    symbol: Option<String>,
}

/// KuCoin Futures OHLCV data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct KfWsOhlcvData {
    symbol: String,
    candles: Vec<String>,
    time: i64,
}

/// WebSocket message wrapper
#[derive(Debug, Deserialize)]
struct KfWsMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    topic: Option<String>,
    subject: Option<String>,
    data: Option<serde_json::Value>,
}

impl KucoinfuturesWs {
    /// 새 KuCoin Futures WebSocket 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            connect_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// 심볼을 KuCoin Futures 마켓 ID로 변환
    fn to_market_id(symbol: &str) -> String {
        // BTC/USD:BTC -> XBTUSDM (perpetual)
        // BTC/USD:USD -> XBTUSDTM (USDT-margined perpetual)
        // ETH/USD:USD -> ETHUSDTM
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return symbol.to_uppercase();
        }

        let base = parts[0];
        let quote_settle: Vec<&str> = parts[1].split(':').collect();

        // Handle BTC -> XBT conversion
        let base_formatted = if base == "BTC" { "XBT" } else { base };

        if quote_settle.len() >= 2 {
            let quote = quote_settle[0];
            let settle = quote_settle[1];

            // Check for expiry date
            let settle_parts: Vec<&str> = settle.split('-').collect();

            if settle_parts.len() > 1 {
                // Delivery futures: XBTMUSDM_240628
                format!("{}{}M_{}", base_formatted, quote, settle_parts[1])
            } else if quote == "USD" && settle == base {
                // Coin-margined perpetual: XBTUSDM
                format!("{}USDM", base_formatted)
            } else {
                // USDT-margined perpetual: XBTUSDTM
                format!("{}USDTM", base_formatted)
            }
        } else {
            format!("{}USDTM", base_formatted)
        }
    }

    /// 마켓 ID를 통합 심볼로 변환
    fn to_unified_symbol(market_id: &str) -> String {
        let upper = market_id.to_uppercase();

        if upper.ends_with("USDM") {
            // Coin-margined: XBTUSDM -> BTC/USD:BTC
            let base = if upper.starts_with("XBT") {
                "BTC"
            } else if upper.starts_with("ETH") {
                "ETH"
            } else {
                let base_end = upper.len() - 4;
                return format!("{}/USD:USD", &upper[..base_end]);
            };
            format!("{}/USD:{}", base, base)
        } else if upper.ends_with("USDTM") {
            // USDT-margined: XBTUSDTM -> BTC/USDT:USDT
            let base = if upper.starts_with("XBT") {
                "BTC"
            } else if upper.starts_with("ETH") {
                "ETH"
            } else {
                let base_end = upper.len() - 5;
                return format!("{}/USDT:USDT", &upper[..base_end]);
            };
            format!("{}/USDT:USDT", base)
        } else {
            market_id.to_string()
        }
    }

    /// Timeframe을 KuCoin Futures 형식으로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1min",
            Timeframe::Minute5 => "5min",
            Timeframe::Minute15 => "15min",
            Timeframe::Minute30 => "30min",
            Timeframe::Hour1 => "1hour",
            Timeframe::Hour2 => "2hour",
            Timeframe::Hour4 => "4hour",
            Timeframe::Hour8 => "8hour",
            Timeframe::Hour12 => "12hour",
            Timeframe::Day1 => "1day",
            Timeframe::Week1 => "1week",
            _ => "1min",
        }
    }

    /// Public 토큰 가져오기 (WebSocket 연결에 필요)
    async fn get_public_token() -> CcxtResult<(String, i64)> {
        let url = "https://api-futures.kucoin.com/api/v1/bullet-public";
        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .send()
            .await
            .map_err(|e| CcxtError::NetworkError { url: url.to_string(), message: e.to_string() })?;

        let token_response: KfWsTokenResponse = response
            .json()
            .await
            .map_err(|e| CcxtError::ParseError { data_type: "KfWsTokenResponse".to_string(), message: e.to_string() })?;

        let data = token_response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get WebSocket token".into(),
        })?;

        let server = data.instance_servers.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No WebSocket server available".into(),
        })?;

        let ws_url = format!("{}?token={}&connectId={}",
            server.endpoint,
            data.token,
            uuid::Uuid::new_v4()
        );

        Ok((ws_url, server.ping_interval))
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &KfWsTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data.ts.map(|t| t / 1_000_000).unwrap_or_else(|| Utc::now().timestamp_millis());

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
            bid: data.best_bid_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.best_bid_size.map(|v| Decimal::from(v)),
            ask: data.best_ask_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.best_ask_size.map(|v| Decimal::from(v)),
            vwap: None,
            open: None,
            close: data.price.as_ref().and_then(|v| v.parse().ok()),
            last: data.price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book_snapshot(data: &KfWsOrderBookSnapshot, market_id: &str) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(market_id);
        let timestamp = data.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

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

        let order_book = OrderBook {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.sequence,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &KfWsTradeData, market_id: &str) -> Trade {
        let symbol = data.symbol.as_ref().map(|s| Self::to_unified_symbol(s))
            .unwrap_or_else(|| Self::to_unified_symbol(market_id));
        let timestamp = data.ts;
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();

        Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol,
            trade_type: None,
            side: Some(data.side.clone()),
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &KfWsOhlcvData, topic: &str) -> Option<WsOhlcvEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);

        // Extract timeframe from topic: /contractMarket/limitCandle:XBTUSDTM_1min
        let timeframe = if let Some(interval_part) = topic.split('_').last() {
            match interval_part {
                "1min" => Timeframe::Minute1,
                "5min" => Timeframe::Minute5,
                "15min" => Timeframe::Minute15,
                "30min" => Timeframe::Minute30,
                "1hour" => Timeframe::Hour1,
                "2hour" => Timeframe::Hour2,
                "4hour" => Timeframe::Hour4,
                "8hour" => Timeframe::Hour8,
                "12hour" => Timeframe::Hour12,
                "1day" => Timeframe::Day1,
                "1week" => Timeframe::Week1,
                _ => Timeframe::Minute1,
            }
        } else {
            Timeframe::Minute1
        };

        // candles: ["1545184800000", "0.0105", "0.0105", "0.0105", "0.0105", "0"]
        // [timestamp, open, close, high, low, volume]
        if data.candles.len() < 6 {
            return None;
        }

        let ohlcv = OHLCV {
            timestamp: data.candles[0].parse().ok()?,
            open: data.candles[1].parse().ok()?,
            high: data.candles[3].parse().ok()?,
            low: data.candles[4].parse().ok()?,
            close: data.candles[2].parse().ok()?,
            volume: data.candles[5].parse().ok()?,
        };

        Some(WsOhlcvEvent { symbol, timeframe, ohlcv })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        let wrapper: KfWsMessage = serde_json::from_str(msg).ok()?;

        // Skip non-message types
        if wrapper.msg_type.as_deref() != Some("message") {
            return None;
        }

        let topic = wrapper.topic.as_ref()?;
        let data = wrapper.data.as_ref()?;
        let subject = wrapper.subject.as_deref();

        // Extract market ID from topic
        let market_id = topic.split(':').last().unwrap_or("");

        match subject {
            Some("ticker") => {
                if let Ok(ticker_data) = serde_json::from_value::<KfWsTickerData>(data.clone()) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(&ticker_data)));
                }
            }
            Some("level2") => {
                if topic.contains("level2Depth") {
                    if let Ok(snapshot) = serde_json::from_value::<KfWsOrderBookSnapshot>(data.clone()) {
                        return Some(WsMessage::OrderBook(Self::parse_order_book_snapshot(&snapshot, market_id)));
                    }
                }
            }
            Some("match") => {
                if let Ok(trade_data) = serde_json::from_value::<KfWsTradeData>(data.clone()) {
                    let trade = Self::parse_trade(&trade_data, market_id);
                    let symbol = trade.symbol.clone();
                    return Some(WsMessage::Trade(WsTradeEvent {
                        symbol,
                        trades: vec![trade],
                    }));
                }
            }
            Some("candle") => {
                if let Ok(ohlcv_data) = serde_json::from_value::<KfWsOhlcvData>(data.clone()) {
                    if let Some(event) = Self::parse_ohlcv(&ohlcv_data, topic) {
                        return Some(WsMessage::Ohlcv(event));
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
        topic: String,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get WebSocket URL with token
        let (ws_url, ping_interval) = Self::get_public_token().await?;

        let mut ws_client = WsClient::new(WsConfig {
            url: ws_url,
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: (ping_interval / 1000) as u64,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let request_id = uuid::Uuid::new_v4().to_string();
        let subscribe_msg = serde_json::json!({
            "id": request_id,
            "type": "subscribe",
            "topic": topic,
            "response": true
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            self.subscriptions.write().await.insert(topic.clone(), topic.clone());
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

impl Default for KucoinfuturesWs {
    fn default() -> Self {
        Self::new(ExchangeConfig::default())
    }
}

#[async_trait]
impl WsExchange for KucoinfuturesWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topic = format!("/contractMarket/ticker:{}", market_id);
        client.subscribe_stream(topic).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_ids: Vec<String> = symbols.iter()
            .map(|s| Self::to_market_id(s))
            .collect();
        let topic = format!("/contractMarket/ticker:{}", market_ids.join(","));
        client.subscribe_stream(topic).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let depth = limit.unwrap_or(50);
        let topic = format!("/contractMarket/level2Depth{}:{}", depth, market_id);
        client.subscribe_stream(topic).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topic = format!("/contractMarket/execution:{}", market_id);
        client.subscribe_stream(topic).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("/contractMarket/limitCandle:{}_{}", market_id, interval);
        client.subscribe_stream(topic).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ref ws_client) = self.ws_client {
            ws_client.close()?;
        }
        self.ws_client = None;
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(ref ws_client) = self.ws_client {
            ws_client.is_connected().await
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_market_id() {
        assert_eq!(KucoinfuturesWs::to_market_id("BTC/USD:BTC"), "XBTUSDM");
        assert_eq!(KucoinfuturesWs::to_market_id("BTC/USDT:USDT"), "XBTUSDTM");
        assert_eq!(KucoinfuturesWs::to_market_id("ETH/USD:ETH"), "ETHUSDM");
        assert_eq!(KucoinfuturesWs::to_market_id("ETH/USDT:USDT"), "ETHUSDTM");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(KucoinfuturesWs::to_unified_symbol("XBTUSDM"), "BTC/USD:BTC");
        assert_eq!(KucoinfuturesWs::to_unified_symbol("XBTUSDTM"), "BTC/USDT:USDT");
        assert_eq!(KucoinfuturesWs::to_unified_symbol("ETHUSDM"), "ETH/USD:ETH");
        assert_eq!(KucoinfuturesWs::to_unified_symbol("ETHUSDTM"), "ETH/USDT:USDT");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(KucoinfuturesWs::format_interval(Timeframe::Minute1), "1min");
        assert_eq!(KucoinfuturesWs::format_interval(Timeframe::Hour1), "1hour");
        assert_eq!(KucoinfuturesWs::format_interval(Timeframe::Day1), "1day");
    }
}
