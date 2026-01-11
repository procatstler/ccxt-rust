//! Exmo WebSocket Implementation
//!
//! Exmo 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    OrderBook, OrderBookEntry, TakerOrMaker, Ticker, Trade, WsExchange, WsMessage,
    WsOrderBookEvent, WsTickerEvent, WsTradeEvent,
};

const WS_PUBLIC_URL: &str = "wss://ws-api.exmo.com:443/v1/public";

/// Exmo WebSocket 클라이언트
pub struct ExmoWs {
    config: ExchangeConfig,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: AtomicI64,
    order_books: Arc<RwLock<HashMap<String, OrderBook>>>,
}

/// Exmo ticker data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct ExmoWsTicker {
    buy_price: Option<String>,
    sell_price: Option<String>,
    last_trade: Option<String>,
    high: Option<String>,
    low: Option<String>,
    avg: Option<String>,
    vol: Option<String>,
    vol_curr: Option<String>,
    updated: Option<i64>,
}

/// Exmo order book data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct ExmoWsOrderBook {
    ask: Option<Vec<Vec<String>>>,
    bid: Option<Vec<Vec<String>>>,
}

/// Exmo trade data from WebSocket
#[derive(Debug, Deserialize, Serialize)]
struct ExmoWsTrade {
    trade_id: Option<i64>,
    #[serde(rename = "type")]
    trade_type: Option<String>,
    price: Option<String>,
    quantity: Option<String>,
    amount: Option<String>,
    date: Option<i64>,
}

/// WebSocket message wrapper
#[derive(Debug, Deserialize)]
struct ExmoWsMessageWrapper {
    ts: Option<i64>,
    event: Option<String>,
    topic: Option<String>,
    data: Option<serde_json::Value>,
    id: Option<i64>,
    code: Option<i32>,
    message: Option<String>,
}

impl ExmoWs {
    /// 새 Exmo WebSocket 클라이언트 생성
    pub fn new(config: ExchangeConfig) -> Self {
        Self {
            config,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: AtomicI64::new(1),
            order_books: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        Self::new(config)
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        self.config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
    }

    /// Get next request ID
    fn next_request_id(&self) -> i64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 심볼을 Exmo 마켓 ID로 변환
    fn to_market_id(symbol: &str) -> String {
        // BTC/USD -> BTC_USD
        symbol.replace('/', "_")
    }

    /// 마켓 ID를 통합 심볼로 변환
    fn to_unified_symbol(market_id: &str) -> String {
        // BTC_USD -> BTC/USD
        market_id.replace('_', "/")
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &ExmoWsTicker, market_id: &str) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(market_id);
        let timestamp = data
            .updated
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|v| v.parse().ok()),
            low: data.low.as_ref().and_then(|v| v.parse().ok()),
            bid: data.buy_price.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.sell_price.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: data.avg.as_ref().and_then(|v| v.parse().ok()),
            open: None,
            close: data.last_trade.as_ref().and_then(|v| v.parse().ok()),
            last: data.last_trade.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: data.avg.as_ref().and_then(|v| v.parse().ok()),
            base_volume: data.vol.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.vol_curr.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(
        data: &ExmoWsOrderBook,
        market_id: &str,
        timestamp: i64,
    ) -> WsOrderBookEvent {
        let symbol = Self::to_unified_symbol(market_id);

        let bids: Vec<OrderBookEntry> = data
            .bid
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| {
                        if e.len() >= 2 {
                            Some(OrderBookEntry {
                                price: e[0].parse().ok()?,
                                amount: e[1].parse().ok()?,
                            })
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let asks: Vec<OrderBookEntry> = data
            .ask
            .as_ref()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| {
                        if e.len() >= 2 {
                            Some(OrderBookEntry {
                                price: e[0].parse().ok()?,
                                amount: e[1].parse().ok()?,
                            })
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

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
            checksum: None,
        };

        WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &ExmoWsTrade, market_id: &str) -> Option<Trade> {
        let symbol = Self::to_unified_symbol(market_id);
        let timestamp = data
            .date
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.as_ref()?.parse().ok()?;
        let amount: Decimal = data.quantity.as_ref()?.parse().ok()?;

        Some(Trade {
            id: data.trade_id.map(|id| id.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol,
            trade_type: None,
            side: data.trade_type.clone(),
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Apply order book update (incremental)
    async fn apply_order_book_update(
        order_books: &RwLock<HashMap<String, OrderBook>>,
        data: &ExmoWsOrderBook,
        market_id: &str,
        timestamp: i64,
    ) -> Option<WsOrderBookEvent> {
        let symbol = Self::to_unified_symbol(market_id);
        let mut books = order_books.write().await;

        if let Some(book) = books.get_mut(&symbol) {
            // Apply bid updates
            if let Some(bids) = &data.bid {
                for entry in bids {
                    if entry.len() >= 2 {
                        if let (Ok(price), Ok(amount)) =
                            (entry[0].parse::<Decimal>(), entry[1].parse::<Decimal>())
                        {
                            // Remove existing entry at this price
                            book.bids.retain(|e| e.price != price);

                            // Add new entry if amount > 0
                            if amount > Decimal::ZERO {
                                book.bids.push(OrderBookEntry { price, amount });
                            }
                        }
                    }
                }
                // Sort bids in descending order
                book.bids.sort_by(|a, b| {
                    b.price
                        .partial_cmp(&a.price)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }

            // Apply ask updates
            if let Some(asks) = &data.ask {
                for entry in asks {
                    if entry.len() >= 2 {
                        if let (Ok(price), Ok(amount)) =
                            (entry[0].parse::<Decimal>(), entry[1].parse::<Decimal>())
                        {
                            // Remove existing entry at this price
                            book.asks.retain(|e| e.price != price);

                            // Add new entry if amount > 0
                            if amount > Decimal::ZERO {
                                book.asks.push(OrderBookEntry { price, amount });
                            }
                        }
                    }
                }
                // Sort asks in ascending order
                book.asks.sort_by(|a, b| {
                    a.price
                        .partial_cmp(&b.price)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }

            book.timestamp = Some(timestamp);
            book.datetime = Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            );

            return Some(WsOrderBookEvent {
                symbol,
                order_book: book.clone(),
                is_snapshot: false,
            });
        }

        None
    }

    /// 메시지 처리
    async fn process_message(
        msg: &str,
        order_books: &RwLock<HashMap<String, OrderBook>>,
    ) -> Option<WsMessage> {
        let wrapper: ExmoWsMessageWrapper = serde_json::from_str(msg).ok()?;

        let event = wrapper.event.as_deref()?;
        let timestamp = wrapper.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        // Handle info and subscribed events
        if event == "info" || event == "subscribed" {
            return None;
        }

        // Handle data events
        if event != "update" && event != "snapshot" {
            return None;
        }

        let topic = wrapper.topic.as_ref()?;
        let data = wrapper.data.as_ref()?;

        // Extract market ID from topic: "spot/ticker:BTC_USD"
        let parts: Vec<&str> = topic.split(':').collect();
        if parts.len() < 2 {
            return None;
        }
        let channel = parts[0];
        let market_id = parts[1];

        match channel {
            "spot/ticker" => {
                if let Ok(ticker_data) = serde_json::from_value::<ExmoWsTicker>(data.clone()) {
                    return Some(WsMessage::Ticker(Self::parse_ticker(
                        &ticker_data,
                        market_id,
                    )));
                }
            },
            "spot/order_book_updates" => {
                if let Ok(ob_data) = serde_json::from_value::<ExmoWsOrderBook>(data.clone()) {
                    if event == "snapshot" {
                        let ob_event = Self::parse_order_book(&ob_data, market_id, timestamp);
                        // Store in cache
                        let symbol = Self::to_unified_symbol(market_id);
                        let mut books = order_books.write().await;
                        books.insert(symbol, ob_event.order_book.clone());
                        return Some(WsMessage::OrderBook(ob_event));
                    } else {
                        // Apply incremental update
                        if let Some(ob_event) = Self::apply_order_book_update(
                            order_books,
                            &ob_data,
                            market_id,
                            timestamp,
                        )
                        .await
                        {
                            return Some(WsMessage::OrderBook(ob_event));
                        }
                    }
                }
            },
            "spot/trades" => {
                if let Ok(trades_data) = serde_json::from_value::<Vec<ExmoWsTrade>>(data.clone()) {
                    let trades: Vec<Trade> = trades_data
                        .iter()
                        .filter_map(|t| Self::parse_trade(t, market_id))
                        .collect();
                    if !trades.is_empty() {
                        let symbol = Self::to_unified_symbol(market_id);
                        return Some(WsMessage::Trade(WsTradeEvent { symbol, trades }));
                    }
                }
            },
            _ => {},
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        topics: Vec<String>,
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

        // Build subscription message
        let request_id = self.next_request_id();
        let subscribe_msg = serde_json::json!({
            "method": "subscribe",
            "topics": topics,
            "id": request_id
        });

        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = topics.join(",");
            self.subscriptions.write().await.insert(key.clone(), key);
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let order_books = Arc::clone(&self.order_books);
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
                        if let Some(ws_msg) = Self::process_message(&msg, &order_books).await {
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

impl Default for ExmoWs {
    fn default() -> Self {
        Self::new(ExchangeConfig::default())
    }
}

#[async_trait]
impl WsExchange for ExmoWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topics = vec![format!("spot/ticker:{}", market_id)];
        client.subscribe_stream(topics).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let topics: Vec<String> = symbols
            .iter()
            .map(|s| format!("spot/ticker:{}", Self::to_market_id(s)))
            .collect();
        client.subscribe_stream(topics).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topics = vec![format!("spot/order_book_updates:{}", market_id)];
        client.subscribe_stream(topics).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new(self.config.clone());
        let market_id = Self::to_market_id(symbol);
        let topics = vec![format!("spot/trades:{}", market_id)];
        client.subscribe_stream(topics).await
    }

    async fn watch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: crate::types::Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Exmo does not support OHLCV WebSocket
        Err(crate::errors::CcxtError::NotSupported {
            feature: "watchOHLCV".to_string(),
        })
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
    use std::str::FromStr;

    #[test]
    fn test_to_market_id() {
        assert_eq!(ExmoWs::to_market_id("BTC/USD"), "BTC_USD");
        assert_eq!(ExmoWs::to_market_id("ETH/USDT"), "ETH_USDT");
        assert_eq!(ExmoWs::to_market_id("XRP/BTC"), "XRP_BTC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(ExmoWs::to_unified_symbol("BTC_USD"), "BTC/USD");
        assert_eq!(ExmoWs::to_unified_symbol("ETH_USDT"), "ETH/USDT");
        assert_eq!(ExmoWs::to_unified_symbol("XRP_BTC"), "XRP/BTC");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = ExmoWsTicker {
            buy_price: Some("30285.84".to_string()),
            sell_price: Some("30299.97".to_string()),
            last_trade: Some("30295.01".to_string()),
            high: Some("30386.7".to_string()),
            low: Some("29542.76".to_string()),
            avg: Some("29974.16".to_string()),
            vol: Some("118.79538518".to_string()),
            vol_curr: Some("3598907.38".to_string()),
            updated: Some(1654205084),
        };

        let event = ExmoWs::parse_ticker(&ticker_data, "BTC_USDT");
        assert_eq!(event.symbol, "BTC/USDT");
        assert_eq!(
            event.ticker.bid,
            Some(Decimal::from_str("30285.84").unwrap())
        );
        assert_eq!(
            event.ticker.ask,
            Some(Decimal::from_str("30299.97").unwrap())
        );
        assert_eq!(
            event.ticker.last,
            Some(Decimal::from_str("30295.01").unwrap())
        );
    }

    #[test]
    fn test_parse_trade() {
        let trade_data = ExmoWsTrade {
            trade_id: Some(389704729),
            trade_type: Some("sell".to_string()),
            price: Some("30310.95".to_string()),
            quantity: Some("0.0197".to_string()),
            amount: Some("597.125715".to_string()),
            date: Some(1654206083),
        };

        let trade = ExmoWs::parse_trade(&trade_data, "BTC_USDT").unwrap();
        assert_eq!(trade.symbol, "BTC/USDT");
        assert_eq!(trade.id, "389704729");
        assert_eq!(trade.side, Some("sell".to_string()));
        assert_eq!(trade.price, Decimal::from_str("30310.95").unwrap());
    }
}
