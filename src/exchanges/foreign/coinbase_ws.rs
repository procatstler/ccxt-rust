//! Coinbase Advanced Trade WebSocket Implementation
//!
//! Coinbase 실시간 데이터 스트리밍

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    TakerOrMaker, Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent,
    WsOrderBookEvent, WsOrderEvent, WsTickerEvent, WsTradeEvent,
};

type HmacSha256 = Hmac<Sha256>;

const WS_PUBLIC_URL: &str = "wss://advanced-trade-ws.coinbase.com";

/// Coinbase WebSocket 클라이언트
pub struct CoinbaseWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    order_book_cache: Arc<RwLock<HashMap<String, OrderBook>>>,
    config: Option<ExchangeConfig>,
}

impl CoinbaseWs {
    /// 새 Coinbase WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            config: None,
        }
    }

    /// Create new Coinbase WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            order_book_cache: Arc::new(RwLock::new(HashMap::new())),
            config: Some(config),
        }
    }

    /// 심볼을 Coinbase 형식으로 변환 (BTC/USDT -> BTC-USDT)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Coinbase 심볼을 통합 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn to_unified_symbol(coinbase_symbol: &str) -> String {
        coinbase_symbol.replace("-", "/")
    }

    /// Generate HMAC-SHA256 signature for WebSocket authentication
    fn generate_signature(
        &self,
        timestamp: &str,
        channel: &str,
        product_ids: &[String],
    ) -> CcxtResult<String> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Config required for authentication".into(),
        })?;

        let secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        // Message format: timestamp + channel + product_ids (comma-separated)
        let products_str = product_ids.join(",");
        let message = format!("{}{}{}", timestamp, channel, products_str);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());

        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Parse order update from user channel
    fn parse_order_update(data: &CoinbaseWsOrderUpdate) -> WsOrderEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);

        let timestamp = data.creation_time
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("open") | Some("OPEN") => OrderStatus::Open,
            Some("pending") | Some("PENDING") => OrderStatus::Open,
            Some("filled") | Some("FILLED") => OrderStatus::Closed,
            Some("cancelled") | Some("CANCELLED") | Some("canceled") | Some("CANCELED") => {
                OrderStatus::Canceled
            }
            Some("expired") | Some("EXPIRED") => OrderStatus::Expired,
            Some("rejected") | Some("REJECTED") | Some("failed") | Some("FAILED") => {
                OrderStatus::Rejected
            }
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("buy") | Some("BUY") => OrderSide::Buy,
            Some("sell") | Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy, // Default to Buy if not specified
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") | Some("LIMIT") => OrderType::Limit,
            Some("market") | Some("MARKET") => OrderType::Market,
            Some("stop") | Some("STOP") | Some("stop_limit") | Some("STOP_LIMIT") => {
                OrderType::StopLimit
            }
            _ => OrderType::Limit, // Default to Limit if not specified
        };

        let amount = data.base_size.unwrap_or_default();
        let filled = data.filled_size.unwrap_or_default();

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.creation_time.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol: unified_symbol,
            order_type,
            time_in_force: None,
            side,
            price: data.limit_price,
            average: data.average_filled_price,
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: data.stop_price,
            trigger_price: data.stop_price,
            stop_loss_price: None,
            take_profit_price: None,
            cost: data.total_value_after_fees,
            trades: Vec::new(),
            fee: data.total_fees.map(|f| Fee {
                cost: Some(f),
                currency: None,
                rate: None,
            }),
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// Parse user trade (fill) from user channel
    fn parse_my_trade(data: &CoinbaseWsFill) -> WsMyTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);

        let timestamp = data.trade_time
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.side.as_deref() {
            Some("buy") | Some("BUY") => Some("buy".to_string()),
            Some("sell") | Some("SELL") => Some("sell".to_string()),
            _ => None,
        };

        let taker_or_maker = match data.liquidity_indicator.as_deref() {
            Some("MAKER") | Some("maker") | Some("M") => Some(TakerOrMaker::Maker),
            Some("TAKER") | Some("taker") | Some("T") => Some(TakerOrMaker::Taker),
            _ => None,
        };

        let price = data.price.unwrap_or_default();
        let amount = data.size.unwrap_or_default();

        let trade = Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.trade_time.clone(),
            symbol: unified_symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost: Some(price * amount),
            fee: data.commission.map(|c| Fee {
                cost: Some(c),
                currency: data.fee_currency.clone(),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol: unified_symbol,
            trades: vec![trade],
        }
    }

    /// Parse balance update from user channel
    fn parse_balance_update(accounts: &[CoinbaseWsAccount]) -> WsBalanceEvent {
        let mut balances = Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
            info: serde_json::to_value(accounts).unwrap_or_default(),
        };

        for account in accounts {
            let currency = account.currency.clone();
            let free = account.available_balance
                .as_ref()
                .and_then(|b| b.value)
                .unwrap_or_default();
            let used = account.hold
                .as_ref()
                .and_then(|h| h.value)
                .unwrap_or_default();
            let total = free + used;

            let balance = Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            };

            balances.currencies.insert(currency, balance);
        }

        WsBalanceEvent { balances }
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &CoinbaseTickerEvent) -> WsTickerEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: unified_symbol.clone(),
            timestamp: Some(timestamp),
            datetime: data.timestamp.clone(),
            high: data.high_24_h,
            low: data.low_24_h,
            bid: data.best_bid,
            bid_volume: data.best_bid_quantity,
            ask: data.best_ask,
            ask_volume: data.best_ask_quantity,
            vwap: None,
            open: None,
            close: data.price,
            last: data.price,
            previous_close: None,
            change: None,
            percentage: data.price_percent_chg_24_h,
            average: None,
            base_volume: data.volume_24_h,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol: unified_symbol, ticker }
    }

    /// 호가창 스냅샷 파싱
    fn parse_order_book_snapshot(data: &CoinbaseLevel2Event) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.updates.iter()
            .filter(|u| u.side.as_deref() == Some("bid"))
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.price_level?,
                    amount: u.new_quantity?,
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.updates.iter()
            .filter(|u| u.side.as_deref() == Some("offer"))
            .filter_map(|u| {
                Some(OrderBookEntry {
                    price: u.price_level?,
                    amount: u.new_quantity?,
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
        };

        WsOrderBookEvent {
            symbol: unified_symbol,
            order_book,
            is_snapshot: true,
        }
    }

    /// 체결 메시지 파싱
    fn parse_trades(data: &CoinbaseMarketTradesEvent) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(&data.product_id);

        let trades: Vec<Trade> = data.trades.iter().map(|t| {
            let timestamp = t.time
                .as_ref()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            Trade {
                id: t.trade_id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: t.time.clone(),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: None,
                price: t.price.unwrap_or_default(),
                amount: t.size.unwrap_or_default(),
                cost: t.price.and_then(|p| t.size.map(|s| p * s)),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
            }
        }).collect();

        WsTradeEvent { symbol: unified_symbol, trades }
    }

    /// 메시지 처리
    fn process_message(msg: &str) -> Option<WsMessage> {
        // Parse the JSON message
        if let Ok(response) = serde_json::from_str::<CoinbaseWsResponse>(msg) {
            match response.channel.as_deref() {
                Some("subscriptions") => {
                    // Subscription confirmation
                    return Some(WsMessage::Subscribed {
                        channel: "subscriptions".to_string(),
                        symbol: None,
                    });
                }
                Some("ticker") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(tickers) = event.tickers {
                                if let Some(ticker_data) = tickers.first() {
                                    return Some(WsMessage::Ticker(Self::parse_ticker(ticker_data)));
                                }
                            }
                        }
                    }
                }
                Some("l2_data") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if event.event_type.as_deref() == Some("snapshot") ||
                               event.event_type.as_deref() == Some("update") {
                                let is_snapshot = event.event_type.as_deref() == Some("snapshot");
                                if let Some(product_id) = &event.product_id {
                                    let unified_symbol = Self::to_unified_symbol(product_id);
                                    let timestamp = Utc::now().timestamp_millis();

                                    let bids: Vec<OrderBookEntry> = event.updates.as_ref()
                                        .map(|updates| updates.iter()
                                            .filter(|u| u.side.as_deref() == Some("bid"))
                                            .filter_map(|u| {
                                                Some(OrderBookEntry {
                                                    price: u.price_level?,
                                                    amount: u.new_quantity?,
                                                })
                                            })
                                            .collect())
                                        .unwrap_or_default();

                                    let asks: Vec<OrderBookEntry> = event.updates.as_ref()
                                        .map(|updates| updates.iter()
                                            .filter(|u| u.side.as_deref() == Some("offer"))
                                            .filter_map(|u| {
                                                Some(OrderBookEntry {
                                                    price: u.price_level?,
                                                    amount: u.new_quantity?,
                                                })
                                            })
                                            .collect())
                                        .unwrap_or_default();

                                    let order_book = OrderBook {
                                        symbol: unified_symbol.clone(),
                                        timestamp: Some(timestamp),
                                        datetime: Some(Utc::now().to_rfc3339()),
                                        nonce: None,
                                        bids,
                                        asks,
                                    };

                                    return Some(WsMessage::OrderBook(WsOrderBookEvent {
                                        symbol: unified_symbol,
                                        order_book,
                                        is_snapshot,
                                    }));
                                }
                            }
                        }
                    }
                }
                Some("market_trades") => {
                    if let Some(events) = response.events {
                        for event in events {
                            if let Some(trades) = &event.trades {
                                if !trades.is_empty() {
                                    let product_id = event.product_id.clone()
                                        .or_else(|| trades.first().and_then(|t| t.product_id.clone()))
                                        .unwrap_or_default();

                                    let unified_symbol = Self::to_unified_symbol(&product_id);

                                    let parsed_trades: Vec<Trade> = trades.iter().map(|t| {
                                        let timestamp = t.time
                                            .as_ref()
                                            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                                            .map(|dt| dt.timestamp_millis())
                                            .unwrap_or_else(|| Utc::now().timestamp_millis());

                                        Trade {
                                            id: t.trade_id.clone().unwrap_or_default(),
                                            order: None,
                                            timestamp: Some(timestamp),
                                            datetime: t.time.clone(),
                                            symbol: unified_symbol.clone(),
                                            trade_type: None,
                                            side: t.side.clone(),
                                            taker_or_maker: None,
                                            price: t.price.unwrap_or_default(),
                                            amount: t.size.unwrap_or_default(),
                                            cost: t.price.and_then(|p| t.size.map(|s| p * s)),
                                            fee: None,
                                            fees: Vec::new(),
                                            info: serde_json::to_value(t).unwrap_or_default(),
                                        }
                                    }).collect();

                                    return Some(WsMessage::Trade(WsTradeEvent {
                                        symbol: unified_symbol,
                                        trades: parsed_trades,
                                    }));
                                }
                            }
                        }
                    }
                }
                Some("user") => {
                    // User channel contains orders, fills, and balance updates
                    if let Some(events) = response.events {
                        for event in events {
                            // Handle order updates
                            if let Some(orders) = &event.orders {
                                for order_data in orders {
                                    return Some(WsMessage::Order(Self::parse_order_update(order_data)));
                                }
                            }
                            // Handle fill (trade) updates
                            if let Some(fills) = &event.fills {
                                for fill_data in fills {
                                    return Some(WsMessage::MyTrade(Self::parse_my_trade(fill_data)));
                                }
                            }
                            // Handle balance/account updates
                            if let Some(accounts) = &event.accounts {
                                if !accounts.is_empty() {
                                    return Some(WsMessage::Balance(Self::parse_balance_update(accounts)));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        product_ids: Vec<String>,
        channel: &str,
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
            "type": "subscribe",
            "product_ids": product_ids,
            "channel": channel
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{}:{}", channel, product_ids.join(","));
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

    /// Subscribe to private channel with authentication
    async fn subscribe_private_channel(
        &mut self,
        product_ids: Vec<String>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key and secret required for private channels".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

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

        // Generate timestamp and signature for authentication
        let timestamp = Utc::now().timestamp().to_string();
        let signature = self.generate_signature(&timestamp, "user", &product_ids)?;

        // Send authenticated subscribe message
        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "product_ids": product_ids,
            "channel": "user",
            "api_key": api_key,
            "timestamp": timestamp,
            "signature": signature
        });
        ws_client.send(&subscribe_msg.to_string())?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            let key = format!("user:{}", product_ids.join(","));
            self.subscriptions.write().await.insert(key, "user".to_string());
        }

        // Event processing task
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

impl Default for CoinbaseWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for CoinbaseWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], "ticker").await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbols: Vec<String> = symbols.iter()
            .map(|s| Self::format_symbol(s))
            .collect();
        client.subscribe_stream(coinbase_symbols, "ticker").await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], "level2").await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let coinbase_symbol = Self::format_symbol(symbol);
        client.subscribe_stream(vec![coinbase_symbol], "market_trades").await
    }

    async fn watch_ohlcv(&self, _symbol: &str, _timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coinbase doesn't support OHLCV via WebSocket
        // Return an error or use ticker to simulate
        Err(crate::errors::CcxtError::NotSupported {
            feature: "Coinbase WebSocket does not support OHLCV streaming".into(),
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

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        // Coinbase uses per-request authentication, not connection-level
        // Authentication happens in subscribe_private_channel
        Ok(())
    }

    async fn watch_orders(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        let product_ids = if let Some(sym) = symbol {
            vec![Self::format_symbol(sym)]
        } else {
            // Subscribe to all products - Coinbase requires at least one product_id
            vec!["BTC-USD".to_string()]
        };

        client.subscribe_private_channel(product_ids).await
    }

    async fn watch_my_trades(&self, symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        let product_ids = if let Some(sym) = symbol {
            vec![Self::format_symbol(sym)]
        } else {
            vec!["BTC-USD".to_string()]
        };

        client.subscribe_private_channel(product_ids).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        // Coinbase user channel provides balance updates for all accounts
        client.subscribe_private_channel(vec!["BTC-USD".to_string()]).await
    }

    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Coinbase spot trading doesn't have positions
        // Return NotSupported error
        Err(CcxtError::NotSupported {
            feature: "Coinbase is a spot exchange and does not support position tracking".into(),
        })
    }
}

// === Coinbase WebSocket Types ===

#[derive(Debug, Deserialize)]
struct CoinbaseWsResponse {
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    events: Option<Vec<CoinbaseWsEvent>>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    sequence_num: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoinbaseWsEvent {
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    tickers: Option<Vec<CoinbaseTickerEvent>>,
    #[serde(default)]
    updates: Option<Vec<CoinbaseLevel2Update>>,
    #[serde(default)]
    trades: Option<Vec<CoinbaseTradeEvent>>,
    // Private channel fields
    #[serde(default)]
    orders: Option<Vec<CoinbaseWsOrderUpdate>>,
    #[serde(default)]
    fills: Option<Vec<CoinbaseWsFill>>,
    #[serde(default)]
    accounts: Option<Vec<CoinbaseWsAccount>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseTickerEvent {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    volume_24_h: Option<Decimal>,
    #[serde(default)]
    low_24_h: Option<Decimal>,
    #[serde(default)]
    high_24_h: Option<Decimal>,
    #[serde(default)]
    low_52_w: Option<Decimal>,
    #[serde(default)]
    high_52_w: Option<Decimal>,
    #[serde(default)]
    price_percent_chg_24_h: Option<Decimal>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    best_bid_quantity: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    best_ask_quantity: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseLevel2Event {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    updates: Vec<CoinbaseLevel2Update>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseLevel2Update {
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price_level: Option<Decimal>,
    #[serde(default)]
    new_quantity: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseMarketTradesEvent {
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    trades: Vec<CoinbaseTradeEvent>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseTradeEvent {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

// === Private Channel Types ===

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
struct CoinbaseWsOrderUpdate {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    creation_time: Option<String>,
    #[serde(default)]
    filled_size: Option<Decimal>,
    #[serde(default)]
    average_filled_price: Option<Decimal>,
    #[serde(default)]
    limit_price: Option<Decimal>,
    #[serde(default)]
    stop_price: Option<Decimal>,
    #[serde(default)]
    base_size: Option<Decimal>,
    #[serde(default)]
    total_fees: Option<Decimal>,
    #[serde(default)]
    total_value_after_fees: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
struct CoinbaseWsFill {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    size: Option<Decimal>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    commission: Option<Decimal>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    liquidity_indicator: Option<String>,
    #[serde(default)]
    trade_time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
struct CoinbaseWsAccount {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    available_balance: Option<CoinbaseWsBalanceValue>,
    #[serde(default)]
    hold: Option<CoinbaseWsBalanceValue>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
struct CoinbaseWsBalanceValue {
    #[serde(default)]
    value: Option<Decimal>,
    #[serde(default)]
    currency: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(CoinbaseWs::format_symbol("BTC/USDT"), "BTC-USDT");
        assert_eq!(CoinbaseWs::format_symbol("ETH/USD"), "ETH-USD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(CoinbaseWs::to_unified_symbol("BTC-USDT"), "BTC/USDT");
        assert_eq!(CoinbaseWs::to_unified_symbol("ETH-USD"), "ETH/USD");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = CoinbaseTickerEvent {
            product_id: "BTC-USD".to_string(),
            price: Some(Decimal::from(50000)),
            volume_24_h: Some(Decimal::from(1000)),
            high_24_h: Some(Decimal::from(51000)),
            low_24_h: Some(Decimal::from(49000)),
            best_bid: Some(Decimal::from(49990)),
            best_ask: Some(Decimal::from(50010)),
            ..Default::default()
        };

        let result = CoinbaseWs::parse_ticker(&ticker_data);
        assert_eq!(result.symbol, "BTC/USD");
        assert_eq!(result.ticker.last, Some(Decimal::from(50000)));
    }

    #[test]
    fn test_with_config_creation() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret");

        let ws = CoinbaseWs::with_config(config);
        assert!(ws.config.is_some());
    }

    #[test]
    fn test_private_data_types() {
        // Test order update deserialization
        let order_json = r#"{
            "order_id": "order123",
            "product_id": "BTC-USD",
            "side": "buy",
            "status": "open",
            "order_type": "limit",
            "filled_size": "0.5",
            "average_filled_price": "50000",
            "limit_price": "51000",
            "base_size": "1.0"
        }"#;
        let order: CoinbaseWsOrderUpdate = serde_json::from_str(order_json).unwrap();
        assert_eq!(order.order_id, Some("order123".to_string()));
        assert_eq!(order.product_id, "BTC-USD");
        assert_eq!(order.side, Some("buy".to_string()));
        assert_eq!(order.status, Some("open".to_string()));

        // Test fill deserialization
        let fill_json = r#"{
            "trade_id": "trade456",
            "order_id": "order123",
            "product_id": "BTC-USD",
            "price": "50000",
            "size": "0.5",
            "side": "buy",
            "commission": "10.0",
            "liquidity_indicator": "MAKER"
        }"#;
        let fill: CoinbaseWsFill = serde_json::from_str(fill_json).unwrap();
        assert_eq!(fill.trade_id, Some("trade456".to_string()));
        assert_eq!(fill.liquidity_indicator, Some("MAKER".to_string()));

        // Test account deserialization
        let account_json = r#"{
            "uuid": "account789",
            "currency": "BTC",
            "available_balance": {
                "value": "1.5",
                "currency": "BTC"
            },
            "hold": {
                "value": "0.5",
                "currency": "BTC"
            }
        }"#;
        let account: CoinbaseWsAccount = serde_json::from_str(account_json).unwrap();
        assert_eq!(account.currency, "BTC");
        assert!(account.available_balance.is_some());
    }

    #[test]
    fn test_parse_order_update() {
        let order_data = CoinbaseWsOrderUpdate {
            order_id: Some("order123".to_string()),
            client_order_id: Some("client456".to_string()),
            product_id: "BTC-USD".to_string(),
            side: Some("buy".to_string()),
            status: Some("open".to_string()),
            order_type: Some("limit".to_string()),
            creation_time: Some("2024-01-15T10:30:00Z".to_string()),
            filled_size: Some(Decimal::from_str("0.5").unwrap()),
            average_filled_price: Some(Decimal::from(50000)),
            limit_price: Some(Decimal::from(51000)),
            stop_price: None,
            base_size: Some(Decimal::from(1)),
            total_fees: Some(Decimal::from_str("10.0").unwrap()),
            total_value_after_fees: Some(Decimal::from(25000)),
        };

        let result = CoinbaseWs::parse_order_update(&order_data);
        assert_eq!(result.order.symbol, "BTC/USD");
        assert_eq!(result.order.id, "order123");
        assert_eq!(result.order.status, OrderStatus::Open);
        assert_eq!(result.order.side, OrderSide::Buy);
        assert_eq!(result.order.order_type, OrderType::Limit);
        assert_eq!(result.order.filled, Decimal::from_str("0.5").unwrap());
    }

    #[test]
    fn test_parse_order_update_filled() {
        let order_data = CoinbaseWsOrderUpdate {
            order_id: Some("order789".to_string()),
            product_id: "ETH-USD".to_string(),
            side: Some("sell".to_string()),
            status: Some("filled".to_string()),
            order_type: Some("market".to_string()),
            filled_size: Some(Decimal::from(10)),
            base_size: Some(Decimal::from(10)),
            ..Default::default()
        };

        let result = CoinbaseWs::parse_order_update(&order_data);
        assert_eq!(result.order.symbol, "ETH/USD");
        assert_eq!(result.order.status, OrderStatus::Closed);
        assert_eq!(result.order.side, OrderSide::Sell);
        assert_eq!(result.order.order_type, OrderType::Market);
    }

    #[test]
    fn test_parse_my_trade() {
        let fill_data = CoinbaseWsFill {
            trade_id: Some("trade123".to_string()),
            order_id: Some("order456".to_string()),
            product_id: "BTC-USD".to_string(),
            price: Some(Decimal::from(50000)),
            size: Some(Decimal::from_str("0.5").unwrap()),
            side: Some("buy".to_string()),
            commission: Some(Decimal::from_str("12.5").unwrap()),
            fee_currency: Some("USD".to_string()),
            liquidity_indicator: Some("MAKER".to_string()),
            trade_time: Some("2024-01-15T10:35:00Z".to_string()),
        };

        let result = CoinbaseWs::parse_my_trade(&fill_data);
        assert_eq!(result.symbol, "BTC/USD");
        assert_eq!(result.trades.len(), 1);
        let trade = &result.trades[0];
        assert_eq!(trade.id, "trade123");
        assert_eq!(trade.price, Decimal::from(50000));
        assert_eq!(trade.amount, Decimal::from_str("0.5").unwrap());
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Maker));
        assert!(trade.fee.is_some());
    }

    #[test]
    fn test_parse_balance_update() {
        let accounts = vec![
            CoinbaseWsAccount {
                uuid: Some("account1".to_string()),
                currency: "BTC".to_string(),
                available_balance: Some(CoinbaseWsBalanceValue {
                    value: Some(Decimal::from_str("1.5").unwrap()),
                    currency: Some("BTC".to_string()),
                }),
                hold: Some(CoinbaseWsBalanceValue {
                    value: Some(Decimal::from_str("0.5").unwrap()),
                    currency: Some("BTC".to_string()),
                }),
            },
            CoinbaseWsAccount {
                uuid: Some("account2".to_string()),
                currency: "USD".to_string(),
                available_balance: Some(CoinbaseWsBalanceValue {
                    value: Some(Decimal::from(10000)),
                    currency: Some("USD".to_string()),
                }),
                hold: Some(CoinbaseWsBalanceValue {
                    value: Some(Decimal::from(5000)),
                    currency: Some("USD".to_string()),
                }),
            },
        ];

        let result = CoinbaseWs::parse_balance_update(&accounts);
        assert!(result.balances.currencies.contains_key("BTC"));
        assert!(result.balances.currencies.contains_key("USD"));

        let btc_balance = result.balances.currencies.get("BTC").unwrap();
        assert_eq!(btc_balance.free, Some(Decimal::from_str("1.5").unwrap()));
        assert_eq!(btc_balance.used, Some(Decimal::from_str("0.5").unwrap()));
        assert_eq!(btc_balance.total, Some(Decimal::from(2)));
    }

    #[test]
    fn test_process_message_user_channel() {
        let msg = r#"{
            "channel": "user",
            "events": [{
                "type": "update",
                "orders": [{
                    "order_id": "test_order",
                    "product_id": "BTC-USD",
                    "side": "buy",
                    "status": "open",
                    "order_type": "limit"
                }]
            }]
        }"#;

        let result = CoinbaseWs::process_message(msg);
        assert!(result.is_some());
        match result.unwrap() {
            WsMessage::Order(order_event) => {
                assert_eq!(order_event.order.symbol, "BTC/USD");
                assert_eq!(order_event.order.id, "test_order");
            }
            _ => panic!("Expected Order message"),
        }
    }

    #[test]
    fn test_process_message_fill() {
        let msg = r#"{
            "channel": "user",
            "events": [{
                "type": "update",
                "fills": [{
                    "trade_id": "test_trade",
                    "order_id": "test_order",
                    "product_id": "ETH-USD",
                    "price": "2000",
                    "size": "1.0",
                    "side": "sell",
                    "liquidity_indicator": "TAKER"
                }]
            }]
        }"#;

        let result = CoinbaseWs::process_message(msg);
        assert!(result.is_some());
        match result.unwrap() {
            WsMessage::MyTrade(trade_event) => {
                assert_eq!(trade_event.symbol, "ETH/USD");
                assert_eq!(trade_event.trades.len(), 1);
                let trade = &trade_event.trades[0];
                assert_eq!(trade.id, "test_trade");
                assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Taker));
            }
            _ => panic!("Expected MyTrade message"),
        }
    }

    #[test]
    fn test_order_status_parsing() {
        // Test various status values
        let statuses = vec![
            ("open", OrderStatus::Open),
            ("OPEN", OrderStatus::Open),
            ("pending", OrderStatus::Open),
            ("filled", OrderStatus::Closed),
            ("FILLED", OrderStatus::Closed),
            ("cancelled", OrderStatus::Canceled),
            ("CANCELLED", OrderStatus::Canceled),
            ("canceled", OrderStatus::Canceled),
            ("expired", OrderStatus::Expired),
            ("rejected", OrderStatus::Rejected),
            ("failed", OrderStatus::Rejected),
        ];

        for (status_str, expected) in statuses {
            let order_data = CoinbaseWsOrderUpdate {
                order_id: Some("test".to_string()),
                product_id: "BTC-USD".to_string(),
                status: Some(status_str.to_string()),
                ..Default::default()
            };
            let result = CoinbaseWs::parse_order_update(&order_data);
            assert_eq!(result.order.status, expected, "Failed for status: {}", status_str);
        }
    }
}
