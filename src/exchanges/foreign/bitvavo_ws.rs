//! Bitvavo WebSocket Implementation
//!
//! Bitvavo 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Exchange, OrderBook, OrderBookEntry, Ticker, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

use super::bitvavo::Bitvavo;

const WS_BASE_URL: &str = "wss://ws.bitvavo.com/v2";

/// Bitvavo WebSocket 클라이언트
pub struct BitvavoWs {
    rest: Bitvavo,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
}

impl BitvavoWs {
    /// REST 거래소 인스턴스를 래핑하는 새 Bitvavo WebSocket 클라이언트 생성
    pub fn new(rest: Bitvavo) -> Self {
        Self {
            rest,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// 심볼을 Bitvavo WebSocket 형식으로 변환 (BTC/EUR -> BTC-EUR)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace('/', "-")
    }

    /// Bitvavo 심볼을 통합 심볼로 변환 (BTC-EUR -> BTC/EUR)
    fn to_unified_symbol(bitvavo_symbol: &str) -> String {
        bitvavo_symbol.replace('-', "/")
    }

    /// Timeframe을 Bitvavo 포맷으로 변환
    fn format_interval(timeframe: Timeframe) -> String {
        match timeframe {
            Timeframe::Minute1 => "1m".to_string(),
            Timeframe::Minute5 => "5m".to_string(),
            Timeframe::Minute15 => "15m".to_string(),
            Timeframe::Minute30 => "30m".to_string(),
            Timeframe::Hour1 => "1h".to_string(),
            Timeframe::Hour2 => "2h".to_string(),
            Timeframe::Hour4 => "4h".to_string(),
            Timeframe::Hour6 => "6h".to_string(),
            Timeframe::Hour8 => "8h".to_string(),
            Timeframe::Hour12 => "12h".to_string(),
            Timeframe::Day1 => "1d".to_string(),
            _ => "1m".to_string(),
        }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &BitvavoWsTicker, market: &str) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(market);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.bid,
            bid_volume: data.bid_size,
            ask: data.ask,
            ask_volume: data.ask_size,
            vwap: None,
            open: data.open,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume,
            quote_volume: data.volume_quote,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent {
            symbol: unified_symbol,
            ticker,
        }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(
        data: &BitvavoWsOrderBook,
        market: &str,
        is_snapshot: bool,
    ) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(market);
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &[Vec<String>]| -> Vec<OrderBookEntry> {
            entries
                .iter()
                .filter_map(|e| {
                    if e.len() >= 2 {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&e[0]).ok()?,
                            amount: Decimal::from_str(&e[1]).ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

        let order_book = OrderBook {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: data.nonce,
            bids,
            asks,
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &BitvavoWsTrade, market: &str) -> Trade {
        let unified_symbol = Self::to_unified_symbol(market);
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Trade {
            id: data.id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: unified_symbol,
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: None,
            price: data.price.unwrap_or_default(),
            amount: data.amount.unwrap_or_default(),
            cost: data
                .price
                .and_then(|p| data.amount.map(|a| p * a)),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 캔들 메시지 파싱
    fn parse_candle(candle: &[serde_json::Value], _market: &str, _interval: &str) -> Option<OHLCV> {
        if candle.len() < 6 {
            return None;
        }

        Some(OHLCV {
            timestamp: candle[0].as_i64()?,
            open: candle[1]
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())?,
            high: candle[2]
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())?,
            low: candle[3]
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())?,
            close: candle[4]
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())?,
            volume: candle[5]
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())?,
        })
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Bitvavo sends JSON objects with "event" field
        let value: serde_json::Value = serde_json::from_str(msg).ok()?;

        let event = value.get("event")?.as_str()?;

        match event {
            "subscribed" => {
                let subscriptions = value.get("subscriptions")?;
                let channels = subscriptions.as_object()?;
                if let Some((channel, _)) = channels.iter().next() {
                    return Some(WsMessage::Subscribed {
                        channel: channel.clone(),
                        symbol: None,
                    });
                }
            }
            "ticker24h" => {
                let data = value.get("data")?.as_array()?;
                if let Some(ticker_data) = data.first() {
                    if let Ok(ticker) = serde_json::from_value::<BitvavoWsTicker>(ticker_data.clone()) {
                        if let Some(market) = ticker.market.as_ref() {
                            return Some(WsMessage::Ticker(Self::parse_ticker(&ticker, market)));
                        }
                    }
                }
            }
            "book" => {
                let market = value.get("market")?.as_str()?;
                if let Ok(ob_data) = serde_json::from_value::<BitvavoWsOrderBook>(value.clone()) {
                    // Bitvavo sends full snapshots when subscribing, then updates
                    let is_snapshot = ob_data.bids.len() > 10 && ob_data.asks.len() > 10;
                    return Some(WsMessage::OrderBook(Self::parse_order_book(
                        &ob_data,
                        market,
                        is_snapshot,
                    )));
                }
            }
            "trade" => {
                let market = value.get("market")?.as_str()?;
                if let Ok(trade) = serde_json::from_value::<BitvavoWsTrade>(value.clone()) {
                    let trades = vec![Self::parse_trade(&trade, market)];
                    return Some(WsMessage::Trade(WsTradeEvent {
                        symbol: Self::to_unified_symbol(market),
                        trades,
                    }));
                }
            }
            "candle" => {
                let market = value.get("market")?.as_str()?;
                let interval = value.get("interval")?.as_str()?;
                let candles = value.get("candle")?.as_array()?;

                if let Some(candle_data) = candles.first() {
                    if let Some(candle_array) = candle_data.as_array() {
                        if let Some(ohlcv) = Self::parse_candle(candle_array, market, interval) {
                            // Convert interval string to Timeframe
                            let timeframe = match interval {
                                "1m" => Timeframe::Minute1,
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
                                _ => Timeframe::Minute1,
                            };

                            return Some(WsMessage::Ohlcv(WsOhlcvEvent {
                                symbol: Self::to_unified_symbol(market),
                                timeframe,
                                ohlcv,
                            }));
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
        channel: &str,
        markets: Vec<String>,
        interval: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_BASE_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let mut subscribe_msg = serde_json::json!({
            "action": "subscribe",
            "channels": [
                {
                    "name": channel,
                    "markets": markets
                }
            ]
        });

        // Add interval for candles
        if let Some(int) = interval {
            subscribe_msg["channels"][0]["interval"] = serde_json::json!([int]);
        }

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, markets.join(","));
            self.subscriptions
                .write()
                .await
                .insert(key, channel.to_string());
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

#[async_trait]
impl WsExchange for BitvavoWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self
            .rest
            .market_id(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        // Clone the rest client for new subscription
        let rest = Bitvavo::new(ExchangeConfig::new())?;
        let mut ws = BitvavoWs::new(rest);
        ws.subscribe_stream("ticker24h", vec![market_id], None)
            .await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self
            .rest
            .market_id(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let rest = Bitvavo::new(ExchangeConfig::new())?;
        let mut ws = BitvavoWs::new(rest);
        ws.subscribe_stream("book", vec![market_id], None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self
            .rest
            .market_id(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let rest = Bitvavo::new(ExchangeConfig::new())?;
        let mut ws = BitvavoWs::new(rest);
        ws.subscribe_stream("trades", vec![market_id], None).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let market_id = self
            .rest
            .market_id(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let interval = Self::format_interval(timeframe);

        let rest = Bitvavo::new(ExchangeConfig::new())?;
        let mut ws = BitvavoWs::new(rest);
        ws.subscribe_stream("candles", vec![market_id], Some(&interval))
            .await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let ws_client = WsClient::new(WsConfig {
                url: WS_BASE_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });

            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = self.ws_client.take() {
            client.close()?;
        }
        self.subscriptions.write().await.clear();
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        if let Some(client) = self.ws_client.as_ref() {
            client.is_connected().await
        } else {
            false
        }
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct BitvavoWsTicker {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default, rename = "bidSize")]
    bid_size: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default, rename = "askSize")]
    ask_size: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default, rename = "volumeQuote")]
    volume_quote: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BitvavoWsOrderBook {
    #[serde(default)]
    market: String,
    #[serde(default)]
    nonce: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitvavoWsTrade {
    #[serde(default)]
    id: String,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        assert_eq!(BitvavoWs::format_symbol("BTC/EUR"), "BTC-EUR");
        assert_eq!(BitvavoWs::to_unified_symbol("BTC-EUR"), "BTC/EUR");
    }

    #[test]
    fn test_interval_conversion() {
        assert_eq!(BitvavoWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(BitvavoWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(BitvavoWs::format_interval(Timeframe::Day1), "1d");
    }
}
