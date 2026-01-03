//! Hyperliquid DEX WebSocket Implementation
//!
//! Hyperliquid 실시간 데이터 스트리밍 (Public & Private Streams)
//!
//! ## Private Channels
//! - `userEvents`: Comprehensive user events (fills, funding, liquidation)
//! - `userFills`: User fill events only
//! - `orderUpdates`: Order status updates
//!
//! Note: Hyperliquid uses wallet address for subscription. Since blockchain
//! data is public, no authentication is required - anyone can subscribe
//! to any user's events using their wallet address.

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, OHLCV,
    WsExchange, WsMessage, WsBalanceEvent, WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent,
};

const WS_BASE_URL: &str = "wss://api.hyperliquid.xyz/ws";
const WS_TESTNET_URL: &str = "wss://api.hyperliquid-testnet.xyz/ws";

/// Hyperliquid WebSocket 클라이언트
pub struct HyperliquidWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    sandbox: bool,
    /// Wallet address for private channel subscriptions (e.g., "0x...")
    wallet_address: Option<String>,
}

impl HyperliquidWs {
    /// 새 Hyperliquid WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: false,
            wallet_address: None,
        }
    }

    /// Testnet WebSocket 클라이언트 생성
    pub fn new_sandbox() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: true,
            wallet_address: None,
        }
    }

    /// Create WebSocket client with wallet address for private channels
    ///
    /// # Arguments
    /// * `address` - Ethereum wallet address (e.g., "0x...")
    ///
    /// # Example
    /// ```ignore
    /// let ws = HyperliquidWs::with_address("0x1234...abcd");
    /// let orders = ws.watch_orders(None).await?;
    /// ```
    pub fn with_address(address: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: false,
            wallet_address: Some(address.to_string()),
        }
    }

    /// Create testnet WebSocket client with wallet address
    pub fn with_address_sandbox(address: &str) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: true,
            wallet_address: Some(address.to_string()),
        }
    }

    /// Set wallet address for private channel subscriptions
    pub fn set_wallet_address(&mut self, address: &str) {
        self.wallet_address = Some(address.to_string());
    }

    /// Get wallet address
    pub fn get_wallet_address(&self) -> Option<&str> {
        self.wallet_address.as_deref()
    }

    /// Get WebSocket URL
    fn get_ws_url(&self) -> &str {
        if self.sandbox {
            WS_TESTNET_URL
        } else {
            WS_BASE_URL
        }
    }

    /// 심볼을 Hyperliquid coin 형식으로 변환
    /// BTC/USDC:USDC -> BTC (for swaps)
    /// PURR/USDC -> PURR/USDC (for spot)
    fn symbol_to_coin(symbol: &str) -> String {
        if symbol.contains(':') {
            // Swap format: BTC/USDC:USDC -> BTC
            symbol.split('/').next().unwrap_or(symbol).to_string()
        } else {
            // Spot format: keep as is
            symbol.to_string()
        }
    }

    /// Hyperliquid coin을 통합 심볼로 변환
    fn coin_to_symbol(coin: &str, is_swap: bool) -> String {
        if is_swap {
            format!("{coin}/USDC:USDC")
        } else {
            coin.to_string()
        }
    }

    /// Timeframe을 Hyperliquid interval로 변환
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute3 => "3m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour2 => "2h",
            Timeframe::Hour4 => "4h",
            Timeframe::Hour8 => "8h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1d",
            Timeframe::Day3 => "3d",
            Timeframe::Week1 => "1w",
            Timeframe::Month1 => "1M",
            _ => "1h", // Default fallback
        }
    }

    /// Parse ticker message (allMids subscription)
    fn parse_ticker_message(data: &HyperliquidAllMids) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        for (coin, mid_price) in &data.mids {
            let symbol = Self::coin_to_symbol(coin, true);

            if let Ok(price) = Decimal::from_str(mid_price) {
                let timestamp = Utc::now().timestamp_millis();

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
                    bid: None,
                    bid_volume: None,
                    ask: None,
                    ask_volume: None,
                    vwap: None,
                    open: None,
                    close: Some(price),
                    last: Some(price),
                    previous_close: None,
                    change: None,
                    percentage: None,
                    average: None,
                    base_volume: None,
                    quote_volume: None,
                    index_price: None,
                    mark_price: Some(price),
                    info: serde_json::to_value(data).unwrap_or_default(),
                };

                messages.push(WsMessage::Ticker(WsTickerEvent { symbol, ticker }));
            }
        }

        messages
    }

    /// Parse order book message (l2Book subscription)
    fn parse_order_book_message(data: &HyperliquidL2Book) -> Option<WsMessage> {
        let coin = &data.coin;
        let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

        let timestamp = data.time.parse::<i64>().ok()
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // Parse bids (first array in levels)
        let bids: Vec<OrderBookEntry> = data.levels.first()
            .map(|level_array| {
                level_array.iter()
                    .filter_map(|level| {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&level.px).ok()?,
                            amount: Decimal::from_str(&level.sz).ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Parse asks (second array in levels)
        let asks: Vec<OrderBookEntry> = data.levels.get(1)
            .map(|level_array| {
                level_array.iter()
                    .filter_map(|level| {
                        Some(OrderBookEntry {
                            price: Decimal::from_str(&level.px).ok()?,
                            amount: Decimal::from_str(&level.sz).ok()?,
                        })
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
        };

        Some(WsMessage::OrderBook(WsOrderBookEvent {
            symbol,
            order_book,
            is_snapshot: true,
        }))
    }

    /// Parse trades message (trades subscription)
    fn parse_trades_message(data: &[HyperliquidTrade]) -> Option<WsMessage> {
        if data.is_empty() {
            return None;
        }

        let coin = &data[0].coin;
        let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

        let trades: Vec<Trade> = data.iter()
            .filter_map(|t| {
                let price = Decimal::from_str(&t.px).ok()?;
                let amount = Decimal::from_str(&t.sz).ok()?;
                let timestamp = t.time;

                Some(Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side: if t.side == "A" {
                        Some("sell".to_string())
                    } else {
                        Some("buy".to_string())
                    },
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                })
            })
            .collect();

        if trades.is_empty() {
            None
        } else {
            Some(WsMessage::Trade(WsTradeEvent { symbol, trades }))
        }
    }

    /// Parse candle message (candle subscription)
    fn parse_candle_message(data: &HyperliquidCandle, timeframe: Timeframe) -> Option<WsMessage> {
        let coin = &data.s;
        let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

        let ohlcv = OHLCV {
            timestamp: data.t,
            open: Decimal::from_str(&data.o).ok()?,
            high: Decimal::from_str(&data.h).ok()?,
            low: Decimal::from_str(&data.l).ok()?,
            close: Decimal::from_str(&data.c).ok()?,
            volume: Decimal::from_str(&data.v).ok()?,
        };

        Some(WsMessage::Ohlcv(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        }))
    }

    // === Private Channel Parse Functions ===

    /// Parse user fills into Trade events (for watch_my_trades)
    fn parse_user_fills(fills: &[HyperliquidFill]) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        // Group fills by coin
        let mut fills_by_coin: HashMap<String, Vec<&HyperliquidFill>> = HashMap::new();
        for fill in fills {
            fills_by_coin.entry(fill.coin.clone()).or_default().push(fill);
        }

        for (coin, coin_fills) in fills_by_coin {
            let symbol = Self::coin_to_symbol(&coin, !coin.contains('/'));

            let trades: Vec<Trade> = coin_fills
                .iter()
                .filter_map(|f| {
                    let price = Decimal::from_str(&f.px).ok()?;
                    let amount = Decimal::from_str(&f.sz).ok()?;
                    let fee = Decimal::from_str(&f.fee).ok();

                    Some(Trade {
                        id: f.tid.to_string(),
                        order: Some(f.oid.to_string()),
                        timestamp: Some(f.time),
                        datetime: Some(
                            chrono::DateTime::from_timestamp_millis(f.time)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default(),
                        ),
                        symbol: symbol.clone(),
                        trade_type: Some(f.dir.clone()),
                        side: if f.side == "A" {
                            Some("sell".to_string())
                        } else {
                            Some("buy".to_string())
                        },
                        taker_or_maker: Some(if f.crossed { TakerOrMaker::Taker } else { TakerOrMaker::Maker }),
                        price,
                        amount,
                        cost: Some(price * amount),
                        fee: fee.map(|f_val| Fee {
                            cost: Some(f_val),
                            currency: Some("USDC".to_string()),
                            rate: None,
                        }),
                        fees: Vec::new(),
                        info: serde_json::to_value(f).unwrap_or_default(),
                    })
                })
                .collect();

            if !trades.is_empty() {
                messages.push(WsMessage::MyTrade(WsMyTradeEvent { symbol, trades }));
            }
        }

        messages
    }

    /// Parse order updates into Order events (for watch_orders)
    fn parse_order_updates(updates: &[HyperliquidOrderUpdate]) -> Vec<WsMessage> {
        updates
            .iter()
            .filter_map(|update| {
                let order = &update.order;
                let coin = &order.coin;
                let symbol = Self::coin_to_symbol(coin, !coin.contains('/'));

                let price = Decimal::from_str(&order.limit_px).ok()?;
                let amount = Decimal::from_str(&order.sz).ok()?;
                let orig_amount = Decimal::from_str(&order.orig_sz).ok()?;
                let filled = orig_amount - amount;

                // Map Hyperliquid status to OrderStatus
                let status = match update.status.as_str() {
                    "open" => OrderStatus::Open,
                    "filled" => OrderStatus::Closed,
                    "canceled" | "marginCanceled" => OrderStatus::Canceled,
                    "triggered" => OrderStatus::Open, // Triggered orders become open
                    "rejected" => OrderStatus::Rejected,
                    _ => OrderStatus::Open,
                };

                // Determine order type
                let order_type = match order.order_type.as_deref() {
                    Some("Market") => OrderType::Market,
                    Some("Stop Market") => OrderType::StopMarket,
                    Some("Take Profit Market") => OrderType::TakeProfitMarket,
                    Some("Stop Limit") => OrderType::StopLimit,
                    Some("Take Profit Limit") => OrderType::TakeProfitLimit,
                    _ => OrderType::Limit, // Default to Limit
                };

                // Determine side
                let side = if order.side == "A" {
                    OrderSide::Sell
                } else {
                    OrderSide::Buy
                };

                // Determine time in force
                let time_in_force = order.tif.as_deref().and_then(|tif| match tif {
                    "Gtc" => Some(TimeInForce::GTC),
                    "Ioc" => Some(TimeInForce::IOC),
                    "Alo" => Some(TimeInForce::PO), // ALO is post-only
                    _ => None,
                });

                Some(WsMessage::Order(WsOrderEvent {
                    order: Order {
                        id: order.oid.to_string(),
                        client_order_id: order.cloid.clone(),
                        timestamp: Some(order.timestamp),
                        datetime: Some(
                            chrono::DateTime::from_timestamp_millis(order.timestamp)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default(),
                        ),
                        last_trade_timestamp: Some(update.status_timestamp),
                        last_update_timestamp: Some(update.status_timestamp),
                        symbol,
                        order_type,
                        time_in_force,
                        post_only: Some(order.tif.as_deref() == Some("Alo")),
                        reduce_only: order.reduce_only,
                        side,
                        price: Some(price),
                        trigger_price: order.trigger_px.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                        amount: orig_amount,
                        cost: Some(price * filled),
                        average: if filled > Decimal::ZERO { Some(price) } else { None },
                        filled,
                        remaining: Some(amount),
                        status,
                        fee: None,
                        trades: Vec::new(),
                        fees: Vec::new(),
                        stop_price: order.trigger_px.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                        take_profit_price: None,
                        stop_loss_price: None,
                        info: serde_json::to_value(update).unwrap_or_default(),
                    },
                }))
            })
            .collect()
    }

    /// Parse funding payment as balance event
    /// Hyperliquid is a USDC-settled perpetual DEX, so balance changes
    /// come from funding payments
    fn parse_funding_as_balance(funding: &HyperliquidFunding) -> WsMessage {
        let timestamp = funding.time;
        let usdc_amount = Decimal::from_str(&funding.usdc).unwrap_or_default();

        let mut currencies = HashMap::new();
        // Funding payment adds or subtracts from USDC balance
        // Positive = received funding, Negative = paid funding
        currencies.insert(
            "USDC".to_string(),
            Balance {
                free: Some(usdc_amount),
                used: None,
                total: Some(usdc_amount),
                debt: None,
            },
        );

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::to_value(funding).unwrap_or_default(),
        };

        WsMessage::Balance(WsBalanceEvent { balances })
    }

    /// Parse user events for positions (extracted from fills)
    /// Note: Hyperliquid doesn't have a dedicated position channel,
    /// positions are derived from fills
    fn parse_positions_from_fills(fills: &[HyperliquidFill]) -> Option<WsMessage> {
        if fills.is_empty() {
            return None;
        }

        // Group by coin and calculate position changes
        let mut positions: Vec<Position> = Vec::new();

        let mut position_by_coin: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        for fill in fills {
            let coin = &fill.coin;
            let size = Decimal::from_str(&fill.sz).unwrap_or_default();
            let closed_pnl = Decimal::from_str(&fill.closed_pnl).unwrap_or_default();

            let entry = position_by_coin.entry(coin.clone()).or_default();
            // Adjust size based on direction
            let signed_size = if fill.dir.contains("Long") {
                if fill.dir.contains("Open") { size } else { -size }
            } else {
                // Short positions
                if fill.dir.contains("Open") { -size } else { size }
            };
            entry.0 += signed_size;
            entry.1 += closed_pnl;
        }

        for (coin, (size_change, realized_pnl)) in position_by_coin {
            let symbol = Self::coin_to_symbol(&coin, !coin.contains('/'));

            positions.push(Position {
                id: None,
                symbol,
                timestamp: Some(fills.last().map(|f| f.time).unwrap_or_default()),
                datetime: None,
                hedged: Some(false),
                side: if size_change > Decimal::ZERO {
                    Some(PositionSide::Long)
                } else if size_change < Decimal::ZERO {
                    Some(PositionSide::Short)
                } else {
                    None
                },
                contracts: Some(size_change.abs()),
                contract_size: Some(Decimal::ONE),
                entry_price: None,
                mark_price: None,
                notional: None,
                leverage: None,
                collateral: None,
                initial_margin: None,
                maintenance_margin: None,
                initial_margin_percentage: None,
                maintenance_margin_percentage: None,
                unrealized_pnl: None,
                liquidation_price: None,
                margin_mode: Some(MarginMode::Cross),
                margin_ratio: None,
                percentage: None,
                info: serde_json::Value::Object(serde_json::Map::new()),
                realized_pnl: Some(realized_pnl),
                last_update_timestamp: None,
                last_price: None,
                stop_loss_price: None,
                take_profit_price: None,
            });
        }

        if positions.is_empty() {
            None
        } else {
            Some(WsMessage::Position(WsPositionEvent { positions }))
        }
    }

    /// Process incoming WebSocket message
    fn process_message(msg: &str, timeframe: Option<Timeframe>) -> Vec<WsMessage> {
        let mut messages = Vec::new();

        // Parse as generic value first to check channel
        let json: serde_json::Value = match serde_json::from_str(msg) {
            Ok(v) => v,
            Err(_) => return messages,
        };

        let channel = json.get("channel").and_then(|c| c.as_str()).unwrap_or("");

        match channel {
            "allMids" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidAllMidsMsg>(json) {
                    messages.extend(Self::parse_ticker_message(&data.data));
                }
            }
            "l2Book" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidL2BookMsg>(json) {
                    if let Some(msg) = Self::parse_order_book_message(&data.data) {
                        messages.push(msg);
                    }
                }
            }
            "trades" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidTradesMsg>(json) {
                    if let Some(msg) = Self::parse_trades_message(&data.data) {
                        messages.push(msg);
                    }
                }
            }
            "candle" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidCandleMsg>(json) {
                    if let Some(tf) = timeframe {
                        if let Some(msg) = Self::parse_candle_message(&data.data, tf) {
                            messages.push(msg);
                        }
                    }
                }
            }
            // === Private Channels ===
            "userEvents" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidUserEventsMsg>(json) {
                    // Parse fills as my trades
                    if !data.data.fills.is_empty() {
                        messages.extend(Self::parse_user_fills(&data.data.fills));
                    }
                    // Also emit position changes from fills
                    if let Some(pos_msg) = Self::parse_positions_from_fills(&data.data.fills) {
                        messages.push(pos_msg);
                    }
                    // Parse funding payments as balance events
                    if let Some(ref funding) = data.data.funding {
                        messages.push(Self::parse_funding_as_balance(funding));
                    }
                }
            }
            "userFills" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidUserFillsMsg>(json) {
                    if !data.data.fills.is_empty() {
                        messages.extend(Self::parse_user_fills(&data.data.fills));
                    }
                }
            }
            "orderUpdates" => {
                if let Ok(data) = serde_json::from_value::<HyperliquidOrderUpdatesMsg>(json) {
                    messages.extend(Self::parse_order_updates(&data.data));
                }
            }
            "error" => {
                if let Some(error_msg) = json.get("data").and_then(|d| d.as_str()) {
                    messages.push(WsMessage::Error(error_msg.to_string()));
                }
            }
            "subscriptionResponse" => {
                // Subscription confirmation
                if let Some(sub_data) = json.get("data") {
                    let channel = sub_data.get("subscription")
                        .and_then(|s| s.get("type"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown");
                    messages.push(WsMessage::Subscribed {
                        channel: channel.to_string(),
                        symbol: None,
                    });
                }
            }
            _ => {
                // Unknown channel - ignore or log
            }
        }

        messages
    }

    /// Subscribe to a channel and return event receiver
    async fn subscribe_channel(
        &mut self,
        subscription: serde_json::Value,
        channel_key: &str,
        timeframe: Option<Timeframe>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = self.get_ws_url();

        // Create WebSocket client
        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 20, // Hyperliquid keepAlive: 20000ms
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Send subscription message
        let sub_msg = serde_json::to_string(&subscription)
            .map_err(|e| CcxtError::ParseError {
                data_type: "subscription".to_string(),
                message: e.to_string(),
            })?;

        ws_client.send(&sub_msg)?;

        self.ws_client = Some(ws_client);

        // Store subscription
        {
            self.subscriptions.write().await.insert(
                channel_key.to_string(),
                serde_json::to_string(&subscription).unwrap_or_default(),
            );
        }

        // Spawn event processing task
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
                        let messages = Self::process_message(&msg, timeframe);
                        for ws_msg in messages {
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

impl Default for HyperliquidWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HyperliquidWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            sandbox: self.sandbox,
            wallet_address: self.wallet_address.clone(),
        }
    }
}

#[async_trait]
impl WsExchange for HyperliquidWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "allMids"
            }
        });

        client.subscribe_channel(subscription, &format!("ticker:{symbol}"), None).await
    }

    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let coin = Self::symbol_to_coin(symbol);

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "l2Book",
                "coin": coin
            }
        });

        client.subscribe_channel(subscription, &format!("orderBook:{symbol}"), None).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let coin = Self::symbol_to_coin(symbol);

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "trades",
                "coin": coin
            }
        });

        client.subscribe_channel(subscription, &format!("trades:{symbol}"), None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = self.clone();

        let coin = Self::symbol_to_coin(symbol);
        let interval = Self::format_interval(timeframe);

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "candle",
                "coin": coin,
                "interval": interval
            }
        });

        client.subscribe_channel(subscription, &format!("candle:{symbol}:{interval}"), Some(timeframe)).await
    }

    // === Private Streams ===

    /// Watch order updates for the configured wallet address
    ///
    /// Subscribes to `orderUpdates` channel which provides real-time
    /// order status changes (open, filled, canceled, triggered, rejected).
    ///
    /// # Requirements
    /// - Wallet address must be set via `with_address()` or `set_wallet_address()`
    ///
    /// # Example
    /// ```ignore
    /// let ws = HyperliquidWs::with_address("0x1234...abcd");
    /// let mut rx = ws.watch_orders(None).await?;
    /// while let Some(msg) = rx.recv().await {
    ///     if let WsMessage::Order(event) = msg {
    ///         println!("Order update: {:?}", event.order);
    ///     }
    /// }
    /// ```
    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let address = self.wallet_address.as_ref().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "Wallet address required for watch_orders. Use with_address() or set_wallet_address().".to_string(),
            }
        })?;

        let mut client = self.clone();

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "orderUpdates",
                "user": address
            }
        });

        client.subscribe_channel(subscription, &format!("orderUpdates:{address}"), None).await
    }

    /// Watch user trade fills for the configured wallet address
    ///
    /// Subscribes to `userFills` channel which provides real-time
    /// fill notifications when orders are executed.
    ///
    /// # Requirements
    /// - Wallet address must be set via `with_address()` or `set_wallet_address()`
    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let address = self.wallet_address.as_ref().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "Wallet address required for watch_my_trades. Use with_address() or set_wallet_address().".to_string(),
            }
        })?;

        let mut client = self.clone();

        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "userFills",
                "user": address
            }
        });

        client.subscribe_channel(subscription, &format!("userFills:{address}"), None).await
    }

    /// Watch position changes for the configured wallet address
    ///
    /// Subscribes to `userEvents` channel which provides comprehensive
    /// user events including fills (from which position changes are derived),
    /// funding payments, and liquidations.
    ///
    /// Note: Hyperliquid doesn't have a dedicated position channel.
    /// Position updates are derived from fill events in the userEvents stream.
    ///
    /// # Requirements
    /// - Wallet address must be set via `with_address()` or `set_wallet_address()`
    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let address = self.wallet_address.as_ref().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "Wallet address required for watch_positions. Use with_address() or set_wallet_address().".to_string(),
            }
        })?;

        let mut client = self.clone();

        // Subscribe to userEvents which includes fills (for position changes),
        // funding, and liquidation events
        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "userEvents",
                "user": address
            }
        });

        client.subscribe_channel(subscription, &format!("userEvents:{address}"), None).await
    }

    /// Watch balance changes for the configured wallet address
    ///
    /// Subscribes to `userEvents` channel which provides funding payments
    /// that affect USDC balance. Hyperliquid is a USDC-settled perpetual DEX,
    /// so all balance changes are in USDC and come from:
    /// - Funding payments (positive = received, negative = paid)
    /// - Realized PnL from trades (tracked via fills)
    ///
    /// Note: For comprehensive balance tracking, you may also want to
    /// monitor fills for fee deductions and realized PnL.
    ///
    /// # Requirements
    /// - Wallet address must be set via `with_address()` or `set_wallet_address()`
    ///
    /// # Example
    /// ```ignore
    /// let ws = HyperliquidWs::with_address("0x1234...abcd");
    /// let mut rx = ws.watch_balance().await?;
    /// while let Some(msg) = rx.recv().await {
    ///     if let WsMessage::Balance(event) = msg {
    ///         if let Some(usdc) = event.balances.currencies.get("USDC") {
    ///             println!("USDC balance change: {:?}", usdc.free);
    ///         }
    ///     }
    /// }
    /// ```
    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let address = self.wallet_address.as_ref().ok_or_else(|| {
            CcxtError::AuthenticationError {
                message: "Wallet address required for watch_balance. Use with_address() or set_wallet_address().".to_string(),
            }
        })?;

        let mut client = self.clone();

        // Subscribe to userEvents which includes funding payments
        // that affect USDC balance
        let subscription = serde_json::json!({
            "method": "subscribe",
            "subscription": {
                "type": "userEvents",
                "user": address
            }
        });

        client.subscribe_channel(subscription, &format!("userEvents:balance:{address}"), None).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Hyperliquid auto-connects on subscription
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

// === Hyperliquid WebSocket Message Types ===

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidAllMidsMsg {
    channel: String,
    data: HyperliquidAllMids,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidAllMids {
    #[serde(default)]
    dex: Option<String>,
    mids: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidL2BookMsg {
    channel: String,
    data: HyperliquidL2Book,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidL2Book {
    coin: String,
    levels: Vec<Vec<HyperliquidLevel>>,
    time: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidLevel {
    px: String,
    sz: String,
    n: i32,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidTradesMsg {
    channel: String,
    data: Vec<HyperliquidTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidTrade {
    coin: String,
    side: String, // "A" for ask/sell, "B" for bid/buy
    px: String,
    sz: String,
    time: i64,
    hash: String,
    tid: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidCandleMsg {
    channel: String,
    data: HyperliquidCandle,
}

#[derive(Debug, Deserialize, Serialize)]
struct HyperliquidCandle {
    t: i64,  // Start time
    #[serde(rename = "T")]
    #[serde(default)]
    end_time: Option<i64>,  // End time
    s: String,  // Symbol
    i: String,  // Interval
    o: String,  // Open
    c: String,  // Close
    h: String,  // High
    l: String,  // Low
    v: String,  // Volume
    #[serde(default)]
    n: Option<i32>,  // Number of trades
}

// === Private Channel Message Types ===

/// userEvents channel message
/// Comprehensive stream for fills, funding, liquidation, nonUserCancel
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidUserEventsMsg {
    channel: String,
    data: HyperliquidUserEventsData,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidUserEventsData {
    #[serde(default)]
    is_snapshot: bool,
    user: String,
    #[serde(default)]
    fills: Vec<HyperliquidFill>,
    #[serde(default)]
    funding: Option<HyperliquidFunding>,
    #[serde(default)]
    liquidation: Option<HyperliquidLiquidation>,
    #[serde(default)]
    non_user_cancel: Option<Vec<HyperliquidNonUserCancel>>,
}

/// userFills channel message
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidUserFillsMsg {
    channel: String,
    data: HyperliquidUserFillsData,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidUserFillsData {
    #[serde(default)]
    is_snapshot: bool,
    user: String,
    fills: Vec<HyperliquidFill>,
}

/// Fill event data
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidFill {
    coin: String,
    px: String,      // Price
    sz: String,      // Size
    side: String,    // "B" for buy, "A" for sell
    time: i64,
    start_position: String,
    dir: String,     // Direction: "Open Long", "Close Long", "Open Short", "Close Short"
    closed_pnl: String,
    hash: String,    // Transaction hash
    oid: i64,        // Order ID
    crossed: bool,   // true = taker
    fee: String,     // Fee amount
    tid: i64,        // Trade ID
    #[serde(default)]
    fee_token: Option<String>,
    #[serde(default)]
    builder_fee: Option<String>,
}

/// Funding event data
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidFunding {
    time: i64,
    coin: String,
    usdc: String,    // Funding payment in USDC
    szi: String,     // Position size
    funding_rate: String,
}

/// Liquidation event data
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidLiquidation {
    lid: i64,        // Liquidation ID
    liquidator: String,
    liquidated_user: String,
    liquidated_ntl_pos: String,
    liquidated_account_value: String,
}

/// Non-user cancel event (e.g., TP/SL triggers)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidNonUserCancel {
    coin: String,
    oid: i64,
}

/// orderUpdates channel message
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderUpdatesMsg {
    channel: String,
    data: Vec<HyperliquidOrderUpdate>,
}

/// Order update data
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderUpdate {
    order: HyperliquidOrderData,
    status: String,        // "open", "filled", "canceled", "triggered", "rejected", "marginCanceled"
    status_timestamp: i64,
}

/// Order data in orderUpdates
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidOrderData {
    coin: String,
    side: String,          // "B" or "A"
    limit_px: String,
    sz: String,
    oid: i64,
    timestamp: i64,
    orig_sz: String,       // Original size
    #[serde(default)]
    cloid: Option<String>, // Client order ID
    #[serde(default)]
    order_type: Option<String>,  // "Limit", "Stop Market", "Take Profit Market", etc.
    #[serde(default)]
    tif: Option<String>,   // Time in force: "Gtc", "Ioc", "Alo"
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    trigger_px: Option<String>,
    #[serde(default)]
    trigger_condition: Option<String>,  // "gt" or "lt"
}

/// Position data for watch_positions
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidPositionData {
    coin: String,
    szi: String,           // Position size (negative for short)
    entry_px: Option<String>,
    position_value: String,
    unrealized_pnl: String,
    #[serde(default)]
    return_on_equity: Option<String>,
    #[serde(default)]
    liquidation_px: Option<String>,
    #[serde(default)]
    margin_used: Option<String>,
    #[serde(default)]
    max_leverage: Option<i32>,
    #[serde(default)]
    leverage: Option<HyperliquidLeverage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyperliquidLeverage {
    #[serde(rename = "type")]
    leverage_type: String,  // "cross" or "isolated"
    value: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_to_coin() {
        assert_eq!(HyperliquidWs::symbol_to_coin("BTC/USDC:USDC"), "BTC");
        assert_eq!(HyperliquidWs::symbol_to_coin("PURR/USDC"), "PURR/USDC");
    }

    #[test]
    fn test_coin_to_symbol() {
        assert_eq!(HyperliquidWs::coin_to_symbol("BTC", true), "BTC/USDC:USDC");
        assert_eq!(HyperliquidWs::coin_to_symbol("PURR/USDC", false), "PURR/USDC");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(HyperliquidWs::format_interval(Timeframe::Minute1), "1m");
        assert_eq!(HyperliquidWs::format_interval(Timeframe::Hour1), "1h");
        assert_eq!(HyperliquidWs::format_interval(Timeframe::Day1), "1d");
    }

    #[test]
    fn test_parse_l2book_message() {
        let json = r#"{
            "channel": "l2Book",
            "data": {
                "coin": "BTC",
                "time": "1710131872708",
                "levels": [
                    [
                        {"px": "68674.0", "sz": "0.97139", "n": 4}
                    ],
                    [
                        {"px": "68675.0", "sz": "0.04396", "n": 1}
                    ]
                ]
            }
        }"#;

        let msg: HyperliquidL2BookMsg = serde_json::from_str(json).unwrap();
        let result = HyperliquidWs::parse_order_book_message(&msg.data);

        assert!(result.is_some());
        if let Some(WsMessage::OrderBook(event)) = result {
            assert_eq!(event.symbol, "BTC/USDC:USDC");
            assert!(!event.order_book.bids.is_empty());
            assert!(!event.order_book.asks.is_empty());
        }
    }

    #[test]
    fn test_parse_trades_message() {
        let trades = vec![
            HyperliquidTrade {
                coin: "BTC".to_string(),
                side: "A".to_string(),
                px: "68517.0".to_string(),
                sz: "0.005".to_string(),
                time: 1710125266669,
                hash: "0xabc123".to_string(),
                tid: 981894269203506,
            }
        ];

        let result = HyperliquidWs::parse_trades_message(&trades);

        assert!(result.is_some());
        if let Some(WsMessage::Trade(event)) = result {
            assert_eq!(event.symbol, "BTC/USDC:USDC");
            assert_eq!(event.trades.len(), 1);
            assert_eq!(event.trades[0].side, Some("sell".to_string()));
        }
    }

    // === Private Channel Tests ===

    #[test]
    fn test_with_address() {
        let ws = HyperliquidWs::with_address("0x1234567890abcdef");
        assert_eq!(ws.get_wallet_address(), Some("0x1234567890abcdef"));
        assert!(!ws.sandbox);
    }

    #[test]
    fn test_with_address_sandbox() {
        let ws = HyperliquidWs::with_address_sandbox("0x1234567890abcdef");
        assert_eq!(ws.get_wallet_address(), Some("0x1234567890abcdef"));
        assert!(ws.sandbox);
    }

    #[test]
    fn test_set_wallet_address() {
        let mut ws = HyperliquidWs::new();
        assert!(ws.get_wallet_address().is_none());

        ws.set_wallet_address("0xabcd");
        assert_eq!(ws.get_wallet_address(), Some("0xabcd"));
    }

    #[test]
    fn test_parse_user_fills() {
        let fills = vec![
            HyperliquidFill {
                coin: "ETH".to_string(),
                px: "3500.50".to_string(),
                sz: "0.1".to_string(),
                side: "B".to_string(),
                time: 1710125266669,
                start_position: "0.0".to_string(),
                dir: "Open Long".to_string(),
                closed_pnl: "0.0".to_string(),
                hash: "0xabc123".to_string(),
                oid: 12345,
                crossed: true,
                fee: "0.35".to_string(),
                tid: 981894269203507,
                fee_token: Some("USDC".to_string()),
                builder_fee: None,
            }
        ];

        let messages = HyperliquidWs::parse_user_fills(&fills);

        assert_eq!(messages.len(), 1);
        if let WsMessage::MyTrade(event) = &messages[0] {
            assert_eq!(event.symbol, "ETH/USDC:USDC");
            assert_eq!(event.trades.len(), 1);
            assert_eq!(event.trades[0].side, Some("buy".to_string()));
            assert_eq!(event.trades[0].taker_or_maker, Some(TakerOrMaker::Taker));
            assert!(event.trades[0].fee.is_some());
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_parse_order_updates() {
        let updates = vec![
            HyperliquidOrderUpdate {
                order: HyperliquidOrderData {
                    coin: "BTC".to_string(),
                    side: "B".to_string(),
                    limit_px: "68500.0".to_string(),
                    sz: "0.01".to_string(),
                    oid: 123456789,
                    timestamp: 1710125266669,
                    orig_sz: "0.05".to_string(),
                    cloid: Some("my-order-1".to_string()),
                    order_type: Some("Limit".to_string()),
                    tif: Some("Gtc".to_string()),
                    reduce_only: Some(false),
                    trigger_px: None,
                    trigger_condition: None,
                },
                status: "open".to_string(),
                status_timestamp: 1710125266670,
            }
        ];

        let messages = HyperliquidWs::parse_order_updates(&updates);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Order(event) = &messages[0] {
            assert_eq!(event.order.id, "123456789");
            assert_eq!(event.order.symbol, "BTC/USDC:USDC");
            assert_eq!(event.order.side, OrderSide::Buy);
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.client_order_id, Some("my-order-1".to_string()));
            assert_eq!(event.order.order_type, OrderType::Limit);
            assert_eq!(event.order.filled, Decimal::from_str("0.04").unwrap()); // 0.05 - 0.01
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_parse_order_updates_filled() {
        let updates = vec![
            HyperliquidOrderUpdate {
                order: HyperliquidOrderData {
                    coin: "ETH".to_string(),
                    side: "A".to_string(),
                    limit_px: "3500.0".to_string(),
                    sz: "0.0".to_string(),  // Fully filled
                    oid: 987654321,
                    timestamp: 1710125266669,
                    orig_sz: "1.0".to_string(),
                    cloid: None,
                    order_type: None,
                    tif: None,
                    reduce_only: None,
                    trigger_px: None,
                    trigger_condition: None,
                },
                status: "filled".to_string(),
                status_timestamp: 1710125266680,
            }
        ];

        let messages = HyperliquidWs::parse_order_updates(&updates);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Order(event) = &messages[0] {
            assert_eq!(event.order.status, OrderStatus::Closed);
            assert_eq!(event.order.side, OrderSide::Sell);
            assert_eq!(event.order.filled, Decimal::from_str("1.0").unwrap());
            assert_eq!(event.order.remaining, Some(Decimal::ZERO));
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_parse_positions_from_fills() {
        let fills = vec![
            HyperliquidFill {
                coin: "BTC".to_string(),
                px: "68500.0".to_string(),
                sz: "0.1".to_string(),
                side: "B".to_string(),
                time: 1710125266669,
                start_position: "0.0".to_string(),
                dir: "Open Long".to_string(),
                closed_pnl: "0.0".to_string(),
                hash: "0xabc".to_string(),
                oid: 123,
                crossed: true,
                fee: "0.1".to_string(),
                tid: 1,
                fee_token: None,
                builder_fee: None,
            },
            HyperliquidFill {
                coin: "BTC".to_string(),
                px: "69000.0".to_string(),
                sz: "0.05".to_string(),
                side: "A".to_string(),
                time: 1710125266670,
                start_position: "0.1".to_string(),
                dir: "Close Long".to_string(),
                closed_pnl: "25.0".to_string(),
                hash: "0xdef".to_string(),
                oid: 124,
                crossed: false,
                fee: "0.05".to_string(),
                tid: 2,
                fee_token: None,
                builder_fee: None,
            },
        ];

        let result = HyperliquidWs::parse_positions_from_fills(&fills);

        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions.len(), 1);
            let pos = &event.positions[0];
            assert_eq!(pos.symbol, "BTC/USDC:USDC");
            assert_eq!(pos.side, Some(PositionSide::Long)); // Net long (0.1 - 0.05 = 0.05)
            assert_eq!(pos.contracts, Some(Decimal::from_str("0.05").unwrap()));
            assert_eq!(pos.realized_pnl, Some(Decimal::from_str("25.0").unwrap()));
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_process_order_updates_message() {
        let json = r#"{
            "channel": "orderUpdates",
            "data": [{
                "order": {
                    "coin": "SOL",
                    "side": "B",
                    "limitPx": "150.0",
                    "sz": "1.0",
                    "oid": 111222333,
                    "timestamp": 1710125266669,
                    "origSz": "2.0"
                },
                "status": "open",
                "statusTimestamp": 1710125266670
            }]
        }"#;

        let messages = HyperliquidWs::process_message(json, None);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Order(event) = &messages[0] {
            assert_eq!(event.order.symbol, "SOL/USDC:USDC");
            assert_eq!(event.order.side, OrderSide::Buy);
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_user_fills_message() {
        let json = r#"{
            "channel": "userFills",
            "data": {
                "isSnapshot": false,
                "user": "0x1234",
                "fills": [{
                    "coin": "DOGE",
                    "px": "0.15",
                    "sz": "1000",
                    "side": "B",
                    "time": 1710125266669,
                    "startPosition": "0",
                    "dir": "Open Long",
                    "closedPnl": "0",
                    "hash": "0xfill",
                    "oid": 999,
                    "crossed": true,
                    "fee": "0.15",
                    "tid": 12345
                }]
            }
        }"#;

        let messages = HyperliquidWs::process_message(json, None);

        assert_eq!(messages.len(), 1);
        if let WsMessage::MyTrade(event) = &messages[0] {
            assert_eq!(event.symbol, "DOGE/USDC:USDC");
            assert_eq!(event.trades.len(), 1);
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_user_events_message() {
        let json = r#"{
            "channel": "userEvents",
            "data": {
                "isSnapshot": false,
                "user": "0x5678",
                "fills": [{
                    "coin": "ARB",
                    "px": "1.20",
                    "sz": "100",
                    "side": "A",
                    "time": 1710125266669,
                    "startPosition": "100",
                    "dir": "Close Long",
                    "closedPnl": "20.0",
                    "hash": "0xevent",
                    "oid": 888,
                    "crossed": false,
                    "fee": "0.12",
                    "tid": 67890
                }]
            }
        }"#;

        let messages = HyperliquidWs::process_message(json, None);

        // Should have both MyTrade and Position events
        assert_eq!(messages.len(), 2);

        let mut has_my_trade = false;
        let mut has_position = false;

        for msg in &messages {
            match msg {
                WsMessage::MyTrade(event) => {
                    assert_eq!(event.symbol, "ARB/USDC:USDC");
                    has_my_trade = true;
                }
                WsMessage::Position(event) => {
                    assert!(!event.positions.is_empty());
                    has_position = true;
                }
                _ => {}
            }
        }

        assert!(has_my_trade, "Expected MyTrade message");
        assert!(has_position, "Expected Position message");
    }

    // === Balance Tests ===

    #[test]
    fn test_parse_funding_as_balance() {
        let funding = HyperliquidFunding {
            time: 1710125266669,
            coin: "BTC".to_string(),
            usdc: "12.50".to_string(),
            szi: "0.1".to_string(),
            funding_rate: "0.0001".to_string(),
        };

        let message = HyperliquidWs::parse_funding_as_balance(&funding);

        if let WsMessage::Balance(event) = message {
            assert!(event.balances.currencies.contains_key("USDC"));
            let usdc = event.balances.currencies.get("USDC").unwrap();
            assert_eq!(usdc.free, Some(Decimal::from_str("12.50").unwrap()));
            assert_eq!(usdc.total, Some(Decimal::from_str("12.50").unwrap()));
            assert_eq!(event.balances.timestamp, Some(1710125266669));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_parse_funding_negative_balance() {
        let funding = HyperliquidFunding {
            time: 1710125266669,
            coin: "ETH".to_string(),
            usdc: "-5.25".to_string(),
            szi: "-1.0".to_string(),
            funding_rate: "-0.00015".to_string(),
        };

        let message = HyperliquidWs::parse_funding_as_balance(&funding);

        if let WsMessage::Balance(event) = message {
            let usdc = event.balances.currencies.get("USDC").unwrap();
            assert_eq!(usdc.free, Some(Decimal::from_str("-5.25").unwrap()));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_user_events_with_funding() {
        let json = r#"{
            "channel": "userEvents",
            "data": {
                "isSnapshot": false,
                "user": "0x1234",
                "fills": [],
                "funding": {
                    "time": 1710125266669,
                    "coin": "BTC",
                    "usdc": "25.00",
                    "szi": "0.5",
                    "fundingRate": "0.0002"
                }
            }
        }"#;

        let messages = HyperliquidWs::process_message(json, None);

        assert_eq!(messages.len(), 1);
        if let WsMessage::Balance(event) = &messages[0] {
            assert!(event.balances.currencies.contains_key("USDC"));
            let usdc = event.balances.currencies.get("USDC").unwrap();
            assert_eq!(usdc.free, Some(Decimal::from_str("25.00").unwrap()));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_user_events_with_fills_and_funding() {
        let json = r#"{
            "channel": "userEvents",
            "data": {
                "isSnapshot": false,
                "user": "0x5678",
                "fills": [{
                    "coin": "SOL",
                    "px": "100.0",
                    "sz": "10",
                    "side": "B",
                    "time": 1710125266669,
                    "startPosition": "0",
                    "dir": "Open Long",
                    "closedPnl": "0",
                    "hash": "0xfill",
                    "oid": 111,
                    "crossed": true,
                    "fee": "1.0",
                    "tid": 222
                }],
                "funding": {
                    "time": 1710125266670,
                    "coin": "SOL",
                    "usdc": "5.50",
                    "szi": "10",
                    "fundingRate": "0.00005"
                }
            }
        }"#;

        let messages = HyperliquidWs::process_message(json, None);

        // Should have MyTrade, Position, and Balance events
        assert_eq!(messages.len(), 3);

        let mut has_my_trade = false;
        let mut has_position = false;
        let mut has_balance = false;

        for msg in &messages {
            match msg {
                WsMessage::MyTrade(event) => {
                    assert_eq!(event.symbol, "SOL/USDC:USDC");
                    has_my_trade = true;
                }
                WsMessage::Position(_) => {
                    has_position = true;
                }
                WsMessage::Balance(event) => {
                    let usdc = event.balances.currencies.get("USDC").unwrap();
                    assert_eq!(usdc.free, Some(Decimal::from_str("5.50").unwrap()));
                    has_balance = true;
                }
                _ => {}
            }
        }

        assert!(has_my_trade, "Expected MyTrade message");
        assert!(has_position, "Expected Position message");
        assert!(has_balance, "Expected Balance message");
    }
}
