//! XT WebSocket Implementation
//!
//! XT Exchange real-time data streaming (Spot & Derivatives)

#![allow(dead_code)]
#![allow(unreachable_patterns)]

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

const WS_SPOT_URL: &str = "wss://stream.xt.com/public";
const WS_CONTRACT_URL: &str = "wss://fstream.xt.com/ws/market";

/// XT WebSocket 클라이언트
///
/// XT Exchange는 Spot과 Derivatives 거래를 지원합니다.
/// - Spot WebSocket: wss://stream.xt.com/public
/// - Contract WebSocket: wss://fstream.xt.com/ws/market
/// - 채널: ticker, tickers, kline, trade, depth, depth_update
pub struct XtWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    use_contract: bool,
}

impl XtWs {
    /// 새 XT WebSocket 클라이언트 생성 (Spot)
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            use_contract: false,
        }
    }

    /// Contract WebSocket 클라이언트 생성
    pub fn new_contract() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            use_contract: true,
        }
    }

    /// 심볼을 XT 형식으로 변환 (BTC/USDT -> btc_usdt)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// XT 심볼을 통합 심볼로 변환 (btc_usdt -> BTC/USDT)
    fn to_unified_symbol(xt_symbol: &str) -> String {
        let parts: Vec<&str> = xt_symbol.split('_').collect();
        if parts.len() >= 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            xt_symbol.to_uppercase()
        }
    }

    /// Parse timeframe string to Timeframe enum
    fn parse_timeframe(tf: &str) -> Timeframe {
        match tf {
            "1m" => Timeframe::Minute1,
            "3m" => Timeframe::Minute3,
            "5m" => Timeframe::Minute5,
            "15m" => Timeframe::Minute15,
            "30m" => Timeframe::Minute30,
            "1h" => Timeframe::Hour1,
            "2h" => Timeframe::Hour2,
            "4h" => Timeframe::Hour4,
            "6h" => Timeframe::Hour6,
            "12h" => Timeframe::Hour12,
            "1d" => Timeframe::Day1,
            "1w" => Timeframe::Week1,
            "1M" => Timeframe::Month1,
            _ => Timeframe::Minute1, // default
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &XtTickerData) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h,
            low: data.l,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.o,
            close: data.c,
            last: data.c,
            previous_close: None,
            change: data.cv,
            percentage: data.cr,
            average: None,
            base_volume: data.q,
            quote_volume: data.v,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// OHLCV 메시지 파싱
    fn parse_ohlcv(data: &XtKlineData, timeframe: &str) -> WsOhlcvEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: data.o.unwrap_or_default(),
            high: data.h.unwrap_or_default(),
            low: data.l.unwrap_or_default(),
            close: data.c.unwrap_or_default(),
            volume: data.q.or(data.a).unwrap_or_default(),
        };

        WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe: Self::parse_timeframe(timeframe),
            ohlcv,
        }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book(data: &XtDepthData, is_snapshot: bool) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data.b.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = entry[0].parse::<Decimal>().ok()?;
                    let amount = entry[1].parse::<Decimal>().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.a.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = entry[0].parse::<Decimal>().ok()?;
                    let amount = entry[1].parse::<Decimal>().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let nonce = data.i.or(data.u);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 호가창 업데이트 적용
    fn apply_order_book_update(cache: &mut OrderBook, data: &XtDepthData) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.map(|t| t as i64).unwrap_or_else(|| Utc::now().timestamp_millis());

        // 비드 업데이트
        for entry in &data.b {
            if entry.len() >= 2 {
                if let (Ok(price), Ok(qty)) = (
                    entry[0].parse::<Decimal>(),
                    entry[1].parse::<Decimal>()
                ) {
                    cache.bids.retain(|e| e.price != price);
                    if qty > Decimal::ZERO {
                        cache.bids.push(OrderBookEntry { price, amount: qty });
                    }
                }
            }
        }
        cache.bids.sort_by(|a, b| b.price.cmp(&a.price));

        // 애스크 업데이트
        for entry in &data.a {
            if entry.len() >= 2 {
                if let (Ok(price), Ok(qty)) = (
                    entry[0].parse::<Decimal>(),
                    entry[1].parse::<Decimal>()
                ) {
                    cache.asks.retain(|e| e.price != price);
                    if qty > Decimal::ZERO {
                        cache.asks.push(OrderBookEntry { price, amount: qty });
                    }
                }
            }
        }
        cache.asks.sort_by(|a, b| a.price.cmp(&b.price));

        cache.timestamp = Some(timestamp);
        cache.datetime = Some(
            chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        cache.nonce = data.i.or(data.u);

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book: cache.clone(),
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &XtTradeData) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.s);
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        // 사이드 결정
        let side = if let Some(b) = data.b {
            if b { Some("buy".to_string()) } else { Some("sell".to_string()) }
        } else if let Some(ref m) = data.m {
            Some(m.to_lowercase())
        } else {
            None
        };

        let trade = Trade {
            id: data.i.clone().unwrap_or_default(),
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
            price: data.p.unwrap_or_default(),
            amount: data.q.or(data.a).unwrap_or_default(),
            cost: data.p.and_then(|p| data.q.or(data.a).map(|q| p * q)),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// 메시지 처리
    fn process_message(msg: &str, order_book_cache: &mut HashMap<String, OrderBook>) -> Option<WsMessage> {
        // Pong 처리
        if msg == "pong" {
            return None;
        }

        if let Ok(response) = serde_json::from_str::<XtWsResponse>(msg) {
            let topic = response.topic.as_deref();
            let event = response.event.as_deref().unwrap_or("");

            match topic {
                Some("ticker") | Some("agg_ticker") => {
                    if let Some(data) = response.data_ticker {
                        return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                    }
                }
                Some("tickers") | Some("agg_tickers") => {
                    if let Some(data_list) = response.data_tickers {
                        if let Some(data) = data_list.first() {
                            return Some(WsMessage::Ticker(Self::parse_ticker(data)));
                        }
                    }
                }
                Some("kline") => {
                    if let Some(data) = response.data_kline {
                        // event에서 timeframe 추출 (예: kline@btc_usdt,5m)
                        let timeframe = event.split(',').last().unwrap_or("1m");
                        return Some(WsMessage::Ohlcv(Self::parse_ohlcv(&data, timeframe)));
                    }
                }
                Some("trade") => {
                    if let Some(data) = response.data_trade {
                        return Some(WsMessage::Trade(Self::parse_trade(&data)));
                    }
                }
                Some("depth") => {
                    if let Some(data) = response.data_depth {
                        let unified_symbol = Self::to_unified_symbol(&data.s);
                        let event = Self::parse_order_book(&data, true);
                        order_book_cache.insert(unified_symbol, event.order_book.clone());
                        return Some(WsMessage::OrderBook(event));
                    }
                }
                Some("depth_update") => {
                    if let Some(data) = response.data_depth {
                        let unified_symbol = Self::to_unified_symbol(&data.s);
                        if let Some(cache) = order_book_cache.get_mut(&unified_symbol) {
                            let event = Self::apply_order_book_update(cache, &data);
                            return Some(WsMessage::OrderBook(event));
                        } else {
                            // 캐시가 없으면 스냅샷으로 처리
                            let event = Self::parse_order_book(&data, false);
                            order_book_cache.insert(unified_symbol, event.order_book.clone());
                            return Some(WsMessage::OrderBook(event));
                        }
                    }
                }
                _ => {}
            }
        }

        // 구독 확인 메시지
        if let Ok(status) = serde_json::from_str::<XtSubscriptionStatus>(msg) {
            if status.code == Some(0) {
                return Some(WsMessage::Subscribed {
                    channel: status.id.unwrap_or_default(),
                    symbol: None,
                });
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channels: Vec<String>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = if self.use_contract { WS_CONTRACT_URL } else { WS_SPOT_URL };

        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let method = if self.use_contract { "SUBSCRIBE" } else { "subscribe" };
        let id = format!("{}{}", Utc::now().timestamp_millis(), channels.join("_"));
        let subscribe_msg = serde_json::json!({
            "method": method,
            "id": id,
            "params": channels
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = channels.join(",");
            self.subscriptions.write().await.insert(key.clone(), key);
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let order_book_cache = Arc::clone(&self.order_book_cache);

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
                        let mut cache = order_book_cache.write().await;
                        if let Some(ws_msg) = Self::process_message(&msg, &mut cache) {
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

impl Default for XtWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for XtWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let xt_symbol = Self::format_symbol(symbol);
        let channel = format!("ticker@{}", xt_symbol);
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_tickers(&self, _symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        // XT는 tickers로 전체 티커를 구독
        client.subscribe_stream(vec!["tickers".to_string()]).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let xt_symbol = Self::format_symbol(symbol);
        let channel = if let Some(l) = limit {
            format!("depth@{},{}", xt_symbol, l)
        } else {
            format!("depth_update@{}", xt_symbol)
        };
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_order_book_for_symbols(&self, symbols: &[&str], limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let channels: Vec<String> = symbols.iter()
            .map(|s| {
                let xt_symbol = Self::format_symbol(s);
                if let Some(l) = limit {
                    format!("depth@{},{}", xt_symbol, l)
                } else {
                    format!("depth_update@{}", xt_symbol)
                }
            })
            .collect();
        client.subscribe_stream(channels).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let xt_symbol = Self::format_symbol(symbol);
        let channel = format!("trade@{}", xt_symbol);
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_trades_for_symbols(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let channels: Vec<String> = symbols.iter()
            .map(|s| format!("trade@{}", Self::format_symbol(s)))
            .collect();
        client.subscribe_stream(channels).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let xt_symbol = Self::format_symbol(symbol);
        let tf_str = match timeframe {
            Timeframe::Second1 => "1m", // XT doesn't support 1s, use 1m
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour3 => "4h", // XT doesn't support 3h, use 4h
            Timeframe::Hour4 => "4h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
        };
        let channel = format!("kline@{},{}", xt_symbol, tf_str);
        client.subscribe_stream(vec![channel]).await
    }

    async fn watch_ohlcv_for_symbols(&self, symbols: &[&str], timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if self.use_contract { Self::new_contract() } else { Self::new() };
        let tf_str = match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour4 => "4h",
            Timeframe::Day1 => "1d",
            _ => "1h",
        };
        let channels: Vec<String> = symbols.iter()
            .map(|s| format!("kline@{},{}", Self::format_symbol(s), tf_str))
            .collect();
        client.subscribe_stream(channels).await
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

// === XT WebSocket Types ===

#[derive(Debug, Deserialize)]
struct XtWsResponse {
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default, rename = "data")]
    data_ticker: Option<XtTickerData>,
    #[serde(default, rename = "data")]
    data_tickers: Option<Vec<XtTickerData>>,
    #[serde(default, rename = "data")]
    data_kline: Option<XtKlineData>,
    #[serde(default, rename = "data")]
    data_trade: Option<XtTradeData>,
    #[serde(default, rename = "data")]
    data_depth: Option<XtDepthData>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
struct XtTickerData {
    #[serde(default)]
    s: String,        // symbol
    #[serde(default)]
    t: Option<i64>,   // timestamp
    #[serde(default)]
    cv: Option<Decimal>, // price change value (spot)
    #[serde(default)]
    cr: Option<Decimal>, // price change rate (spot)
    #[serde(default)]
    ch: Option<Decimal>, // price change (contract)
    #[serde(default)]
    o: Option<Decimal>,  // open
    #[serde(default)]
    c: Option<Decimal>,  // close
    #[serde(default)]
    h: Option<Decimal>,  // high
    #[serde(default)]
    l: Option<Decimal>,  // low
    #[serde(default)]
    q: Option<Decimal>,  // quantity (base volume)
    #[serde(default)]
    v: Option<Decimal>,  // volume (quote volume)
    #[serde(default)]
    a: Option<Decimal>,  // amount (contract)
    #[serde(default)]
    i: Option<Decimal>,  // index price
    #[serde(default)]
    m: Option<Decimal>,  // mark price
    #[serde(default)]
    bp: Option<Decimal>, // best bid
    #[serde(default)]
    ap: Option<Decimal>, // best ask
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct XtKlineData {
    #[serde(default)]
    s: String,        // symbol
    #[serde(default)]
    t: Option<i64>,   // timestamp
    #[serde(default)]
    i: Option<String>, // interval
    #[serde(default)]
    o: Option<Decimal>,
    #[serde(default)]
    h: Option<Decimal>,
    #[serde(default)]
    l: Option<Decimal>,
    #[serde(default)]
    c: Option<Decimal>,
    #[serde(default)]
    q: Option<Decimal>,  // quantity (spot)
    #[serde(default)]
    v: Option<Decimal>,  // volume
    #[serde(default)]
    a: Option<Decimal>,  // amount (contract)
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct XtTradeData {
    #[serde(default)]
    s: String,        // symbol
    #[serde(default)]
    i: Option<String>, // trade id
    #[serde(default)]
    t: Option<i64>,   // timestamp
    #[serde(default)]
    p: Option<Decimal>, // price
    #[serde(default)]
    q: Option<Decimal>, // quantity (spot)
    #[serde(default)]
    a: Option<Decimal>, // amount (contract)
    #[serde(default)]
    b: Option<bool>,    // is buy (spot)
    #[serde(default)]
    m: Option<String>,  // side: BID/ASK (contract)
}

#[derive(Debug, Default, Deserialize)]
struct XtDepthData {
    #[serde(default)]
    s: String,        // symbol
    #[serde(default)]
    fi: Option<i64>,  // first update id (spot)
    #[serde(default)]
    i: Option<i64>,   // update id (spot)
    #[serde(default)]
    pu: Option<i64>,  // previous update id (contract)
    #[serde(default)]
    fu: Option<i64>,  // first update id (contract)
    #[serde(default)]
    u: Option<i64>,   // update id (contract)
    #[serde(default)]
    t: Option<i64>,   // timestamp
    #[serde(default)]
    a: Vec<Vec<String>>, // asks [[price, qty], ...]
    #[serde(default)]
    b: Vec<Vec<String>>, // bids [[price, qty], ...]
}

#[derive(Debug, Default, Deserialize)]
struct XtSubscriptionStatus {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    method: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xt_ws_creation() {
        let _ws = XtWs::new();
        let _ws_contract = XtWs::new_contract();
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(XtWs::format_symbol("BTC/USDT"), "btc_usdt");
        assert_eq!(XtWs::format_symbol("ETH/BTC"), "eth_btc");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(XtWs::to_unified_symbol("btc_usdt"), "BTC/USDT");
        assert_eq!(XtWs::to_unified_symbol("eth_btc"), "ETH/BTC");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = XtTickerData {
            s: "btc_usdt".to_string(),
            t: Some(1683501935877),
            o: Some(Decimal::from(28823)),
            c: Some(Decimal::from(28741)),
            h: Some(Decimal::from(29137)),
            l: Some(Decimal::from(28660)),
            q: Some(Decimal::from(6372)),
            v: Some(Decimal::from(184086075)),
            ..Default::default()
        };

        let result = XtWs::parse_ticker(&ticker_data);
        assert_eq!(result.symbol, "BTC/USDT");
        assert_eq!(result.ticker.last, Some(Decimal::from(28741)));
    }
}
