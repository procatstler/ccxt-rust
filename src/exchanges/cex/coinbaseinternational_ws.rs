//! Coinbase International WebSocket Implementation
//!
//! Coinbase International Exchange real-time data streaming

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
    OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, WsExchange, WsMessage, WsOrderBookEvent,
    WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws-md.international.coinbase.com";

/// Coinbase International WebSocket 클라이언트
///
/// Coinbase International Exchange는 국제 규제 준수 거래소입니다.
/// - WebSocket URL: wss://ws-md.international.coinbase.com
/// - 채널: INSTRUMENTS, MATCH, MBO (Market By Order), MBP (Market By Price)
pub struct CoinbaseInternationalWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
}

impl CoinbaseInternationalWs {
    /// 새 Coinbase International WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 심볼을 Coinbase International 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Coinbase International 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(coinbase_symbol: &str) -> String {
        coinbase_symbol.replace("-", "/")
    }

    /// 인스트루먼트 데이터에서 티커 파싱
    fn parse_instrument_ticker(data: &CoinbaseIntlInstrumentData) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_id);
        let timestamp = data
            .event_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price_24h,
            low: data.low_price_24h,
            bid: data.best_bid_price,
            bid_volume: data.best_bid_qty,
            ask: data.best_ask_price,
            ask_volume: data.best_ask_qty,
            vwap: None,
            open: data.open_price_24h,
            close: data.last_price,
            last: data.last_price,
            previous_close: None,
            change: None,
            percentage: data.price_change_percent_24h,
            average: None,
            base_volume: data.base_volume_24h,
            quote_volume: data.quote_volume_24h,
            index_price: data.index_price,
            mark_price: data.mark_price,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// MBP (Market By Price) 스냅샷 파싱
    fn parse_mbp_snapshot(data: &CoinbaseIntlMbpData) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_id);
        let timestamp = data
            .event_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|entry| {
                Some(OrderBookEntry {
                    price: entry.price?,
                    amount: entry.qty?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|entry| {
                Some(OrderBookEntry {
                    price: entry.price?,
                    amount: entry.qty?,
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
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// MBP 업데이트 파싱
    fn parse_mbp_update(data: &CoinbaseIntlMbpData, cache: &mut OrderBook) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_id);
        let timestamp = data
            .event_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // 업데이트 적용
        for bid in &data.bids {
            if let (Some(price), Some(qty)) = (bid.price, bid.qty) {
                cache.bids.retain(|e| e.price != price);
                if qty > Decimal::ZERO {
                    cache.bids.push(OrderBookEntry { price, amount: qty });
                }
            }
        }
        cache.bids.sort_by(|a, b| b.price.cmp(&a.price));

        for ask in &data.asks {
            if let (Some(price), Some(qty)) = (ask.price, ask.qty) {
                cache.asks.retain(|e| e.price != price);
                if qty > Decimal::ZERO {
                    cache.asks.push(OrderBookEntry { price, amount: qty });
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

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book: cache.clone(),
            is_snapshot: false,
        }
    }

    /// 체결 메시지 파싱
    fn parse_match(data: &CoinbaseIntlMatchData) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.instrument_id);
        let timestamp = data
            .event_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let trade = Trade {
            id: data.match_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: None,
            price: data.price.unwrap_or_default(),
            amount: data.qty.unwrap_or_default(),
            cost: data.price.and_then(|p| data.qty.map(|q| p * q)),
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
    fn process_message(
        msg: &str,
        order_book_cache: &mut HashMap<String, OrderBook>,
    ) -> Option<WsMessage> {
        if let Ok(response) = serde_json::from_str::<CoinbaseIntlWsResponse>(msg) {
            match response.channel.as_deref() {
                Some("SUBSCRIPTIONS") => {
                    return Some(WsMessage::Subscribed {
                        channel: "subscriptions".to_string(),
                        symbol: None,
                    });
                },
                Some("INSTRUMENTS") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(instruments) = event.instruments {
                                if let Some(instrument) = instruments.first() {
                                    return Some(WsMessage::Ticker(Self::parse_instrument_ticker(
                                        instrument,
                                    )));
                                }
                            }
                        }
                    }
                },
                Some("MBP") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(mbp_data) = &event.mbp {
                                let unified_symbol =
                                    Self::to_unified_symbol(&mbp_data.instrument_id);
                                let is_snapshot = event.event_type.as_deref() == Some("SNAPSHOT");

                                if is_snapshot {
                                    let event = Self::parse_mbp_snapshot(mbp_data);
                                    order_book_cache
                                        .insert(unified_symbol.clone(), event.order_book.clone());
                                    return Some(WsMessage::OrderBook(event));
                                } else if let Some(cache) =
                                    order_book_cache.get_mut(&unified_symbol)
                                {
                                    let event = Self::parse_mbp_update(mbp_data, cache);
                                    return Some(WsMessage::OrderBook(event));
                                }
                            }
                        }
                    }
                },
                Some("MATCH") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(matches) = event.matches {
                                if let Some(match_data) = matches.first() {
                                    return Some(WsMessage::Trade(Self::parse_match(match_data)));
                                }
                            }
                        }
                    }
                },
                _ => {},
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        instrument_ids: Vec<String>,
        channels: Vec<String>,
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
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // 구독 메시지 전송
        let subscribe_msg = serde_json::json!({
            "type": "SUBSCRIBE",
            "instrument_ids": instrument_ids,
            "channels": channels
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channels.join(","), instrument_ids.join(","));
            self.subscriptions
                .write()
                .await
                .insert(key, channels.join(","));
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let order_book_cache = Arc::clone(&self.order_book_cache);

        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    },
                    WsEvent::Disconnected => {
                        let _ = tx.send(WsMessage::Disconnected);
                    },
                    WsEvent::Message(msg) => {
                        let mut cache = order_book_cache.write().await;
                        if let Some(ws_msg) = Self::process_message(&msg, &mut cache) {
                            let _ = tx.send(ws_msg);
                        }
                    },
                    WsEvent::Error(err) => {
                        let _ = tx.send(WsMessage::Error(err));
                    },
                    _ => {},
                }
            }
        });

        Ok(event_rx)
    }
}

impl Default for CoinbaseInternationalWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinbaseInternationalWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument_id = Self::format_symbol(symbol);
        client
            .subscribe_stream(vec![instrument_id], vec!["INSTRUMENTS".to_string()])
            .await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client
            .subscribe_stream(instrument_ids, vec!["INSTRUMENTS".to_string()])
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument_id = Self::format_symbol(symbol);
        client
            .subscribe_stream(vec![instrument_id], vec!["MBP".to_string()])
            .await
    }

    async fn watch_order_book_for_symbols(
        &self,
        symbols: &[&str],
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client
            .subscribe_stream(instrument_ids, vec!["MBP".to_string()])
            .await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument_id = Self::format_symbol(symbol);
        client
            .subscribe_stream(vec![instrument_id], vec!["MATCH".to_string()])
            .await
    }

    async fn watch_trades_for_symbols(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument_ids: Vec<String> = symbols.iter().map(|s| Self::format_symbol(s)).collect();
        client
            .subscribe_stream(instrument_ids, vec!["MATCH".to_string()])
            .await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "Coinbase International WebSocket does not support OHLCV streaming".into(),
        })
    }

    async fn watch_ohlcv_for_symbols(
        &self,
        _symbols: &[&str],
        _timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        Err(crate::errors::CcxtError::NotSupported {
            feature: "Coinbase International WebSocket does not support OHLCV streaming".into(),
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

// === Coinbase International WebSocket Types ===

#[derive(Debug, Deserialize)]
struct CoinbaseIntlWsResponse {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    events: Option<Vec<CoinbaseIntlWsEvent>>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    sequence_num: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoinbaseIntlWsEvent {
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    instruments: Option<Vec<CoinbaseIntlInstrumentData>>,
    #[serde(default)]
    mbp: Option<CoinbaseIntlMbpData>,
    #[serde(default)]
    matches: Option<Vec<CoinbaseIntlMatchData>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlInstrumentData {
    #[serde(default)]
    instrument_id: String,
    #[serde(default)]
    instrument_type: Option<String>,
    #[serde(default)]
    base_asset: Option<String>,
    #[serde(default)]
    quote_asset: Option<String>,
    #[serde(default)]
    last_price: Option<Decimal>,
    #[serde(default)]
    open_price_24h: Option<Decimal>,
    #[serde(default)]
    high_price_24h: Option<Decimal>,
    #[serde(default)]
    low_price_24h: Option<Decimal>,
    #[serde(default)]
    price_change_percent_24h: Option<Decimal>,
    #[serde(default)]
    base_volume_24h: Option<Decimal>,
    #[serde(default)]
    quote_volume_24h: Option<Decimal>,
    #[serde(default)]
    best_bid_price: Option<Decimal>,
    #[serde(default)]
    best_bid_qty: Option<Decimal>,
    #[serde(default)]
    best_ask_price: Option<Decimal>,
    #[serde(default)]
    best_ask_qty: Option<Decimal>,
    #[serde(default)]
    index_price: Option<Decimal>,
    #[serde(default)]
    mark_price: Option<Decimal>,
    #[serde(default)]
    event_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlMbpData {
    #[serde(default)]
    instrument_id: String,
    #[serde(default)]
    bids: Vec<CoinbaseIntlPriceLevel>,
    #[serde(default)]
    asks: Vec<CoinbaseIntlPriceLevel>,
    #[serde(default)]
    event_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseIntlPriceLevel {
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    qty: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseIntlMatchData {
    #[serde(default)]
    match_id: Option<String>,
    #[serde(default)]
    instrument_id: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    qty: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    event_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coinbaseinternational_ws_creation() {
        let _ws = CoinbaseInternationalWs::new();
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinbaseInternationalWs::format_symbol("BTC/USD"), "BTC-USD");
        assert_eq!(
            CoinbaseInternationalWs::format_symbol("ETH/USDT"),
            "ETH-USDT"
        );
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(
            CoinbaseInternationalWs::to_unified_symbol("BTC-USD"),
            "BTC/USD"
        );
        assert_eq!(
            CoinbaseInternationalWs::to_unified_symbol("ETH-USDT"),
            "ETH/USDT"
        );
    }

    #[test]
    fn test_parse_instrument_ticker() {
        let instrument_data = CoinbaseIntlInstrumentData {
            instrument_id: "BTC-USD".to_string(),
            last_price: Some(Decimal::from(50000)),
            base_volume_24h: Some(Decimal::from(1000)),
            high_price_24h: Some(Decimal::from(51000)),
            low_price_24h: Some(Decimal::from(49000)),
            best_bid_price: Some(Decimal::from(49990)),
            best_ask_price: Some(Decimal::from(50010)),
            ..Default::default()
        };

        let result = CoinbaseInternationalWs::parse_instrument_ticker(&instrument_data);
        assert_eq!(result.symbol, "BTC/USD");
        assert_eq!(result.ticker.last, Some(Decimal::from(50000)));
    }
}
