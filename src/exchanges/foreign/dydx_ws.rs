//! dYdX WebSocket Implementation
//!
//! dYdX v4 real-time data streaming
//!
//! # Public Channels
//! - `v4_markets` - Market ticker updates
//! - `v4_orderbook` - Order book updates
//! - `v4_trades` - Trade updates
//! - `v4_candles` - OHLCV candle updates
//!
//! # Private Channels (requires subaccount address)
//! - `v4_subaccounts` - Account, position, order, and fill updates

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{WsClient, WsConfig, WsEvent};
use crate::errors::CcxtResult;
use crate::types::{
    Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Position, PositionSide, MarginMode,
    Ticker, TimeInForce, Timeframe, Trade, TakerOrMaker,
    WsExchange, WsMessage, WsTickerEvent, WsOrderBookEvent, WsTradeEvent, WsOhlcvEvent,
    WsOrderEvent, WsPositionEvent, WsMyTradeEvent, OHLCV,
};

const WS_PUBLIC_URL: &str = "wss://indexer.dydx.trade/v4/ws";
const WS_TESTNET_URL: &str = "wss://indexer.v4testnet.dydx.exchange/v4/ws";

/// dYdX WebSocket client
pub struct DydxWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    /// 테스트넷 모드
    testnet: bool,
    /// Subaccount 주소 (private 채널용)
    /// Format: "dydx1xxx.../0" (address/subaccount_number)
    subaccount_id: Option<String>,
}

impl DydxWs {
    /// Create new dYdX WebSocket client (Mainnet)
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            testnet: false,
            subaccount_id: None,
        }
    }

    /// Create new dYdX WebSocket client for testnet
    pub fn testnet() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            testnet: true,
            subaccount_id: None,
        }
    }

    /// Create dYdX WebSocket client with subaccount for private channels
    ///
    /// # Arguments
    ///
    /// * `address` - dYdX address (e.g., "dydx1...")
    /// * `subaccount_number` - Subaccount number (default: 0)
    /// * `testnet` - Use testnet endpoints
    pub fn with_subaccount(address: &str, subaccount_number: u32, testnet: bool) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            testnet,
            subaccount_id: Some(format!("{}/{}", address, subaccount_number)),
        }
    }

    /// Get WebSocket URL based on network
    fn get_ws_url(&self) -> &'static str {
        if self.testnet {
            WS_TESTNET_URL
        } else {
            WS_PUBLIC_URL
        }
    }

    /// Convert symbol to dYdX WebSocket format (BTC/USD -> BTC-USD)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert dYdX symbol to unified format (BTC-USD -> BTC/USD)
    fn to_unified_symbol(dydx_symbol: &str) -> String {
        dydx_symbol.replace("-", "/")
    }

    /// Parse ticker message from v4_markets channel
    fn parse_ticker(data: &DydxMarketData) -> Option<WsTickerEvent> {
        let symbol = Self::to_unified_symbol(data.ticker.as_ref()?);
        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: data.low_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: None,
            ask: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            last: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            quote_volume: None,
            index_price: data.index_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            mark_price: data.oracle_price.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTickerEvent { symbol, ticker })
    }

    /// Parse order book message from v4_orderbook channel
    fn parse_order_book(data: &DydxOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<DydxOrderBookLevel>| -> Vec<OrderBookEntry> {
            entries.iter().filter_map(|e| {
                Some(OrderBookEntry {
                    price: Decimal::from_str(&e.price).ok()?,
                    amount: Decimal::from_str(&e.size).ok()?,
                })
            }).collect()
        };

        let bids = parse_entries(&data.bids);
        let asks = parse_entries(&data.asks);

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

    /// Parse trade message from v4_trades channel
    fn parse_trades(data: &DydxTradeData, symbol: &str) -> WsTradeEvent {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let trades: Vec<Trade> = data.trades.iter().filter_map(|t| {
            let timestamp = t.created_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let price = Decimal::from_str(&t.price).ok()?;
            let amount = Decimal::from_str(&t.size).ok()?;

            Some(Trade {
                id: t.id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: unified_symbol.clone(),
                trade_type: None,
                side: t.side.clone(),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
            })
        }).collect();

        WsTradeEvent {
            symbol: unified_symbol,
            trades,
        }
    }

    /// Parse OHLCV message from v4_candles channel
    fn parse_candles(data: &DydxCandleData, symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let unified_symbol = Self::to_unified_symbol(symbol);

        let candle = data.candles.first()?;
        let timestamp = candle.started_at.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())?;

        let ohlcv = OHLCV {
            timestamp,
            open: Decimal::from_str(&candle.open).ok()?,
            high: Decimal::from_str(&candle.high).ok()?,
            low: Decimal::from_str(&candle.low).ok()?,
            close: Decimal::from_str(&candle.close).ok()?,
            volume: Decimal::from_str(&candle.base_token_volume).ok()?,
        };

        Some(WsOhlcvEvent {
            symbol: unified_symbol,
            timeframe,
            ohlcv,
        })
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: DydxWsMessage = serde_json::from_str(msg).ok()?;

        match json.msg_type.as_str() {
            "connected" => Some(WsMessage::Connected),
            "subscribed" => {
                Some(WsMessage::Subscribed {
                    channel: json.channel?,
                    symbol: json.id.clone(),
                })
            }
            "channel_data" => {
                let channel = json.channel.as_ref()?;
                let contents = json.contents.as_ref()?;

                if channel == "v4_markets" {
                    if let Ok(market_data) = serde_json::from_value::<DydxMarketData>(contents.clone()) {
                        return Self::parse_ticker(&market_data).map(WsMessage::Ticker);
                    }
                } else if channel == "v4_orderbook" {
                    if let Ok(ob_data) = serde_json::from_value::<DydxOrderBookData>(contents.clone()) {
                        let symbol = json.id.as_ref()?;
                        return Some(WsMessage::OrderBook(Self::parse_order_book(&ob_data, symbol)));
                    }
                } else if channel == "v4_trades" {
                    if let Ok(trade_data) = serde_json::from_value::<DydxTradeData>(contents.clone()) {
                        let symbol = json.id.as_ref()?;
                        return Some(WsMessage::Trade(Self::parse_trades(&trade_data, symbol)));
                    }
                } else if channel == "v4_candles" {
                    if let Ok(candle_data) = serde_json::from_value::<DydxCandleData>(contents.clone()) {
                        // Extract symbol and timeframe from id (format: "BTC-USD/1MIN")
                        let id_parts: Vec<&str> = json.id.as_ref()?.split('/').collect();
                        if id_parts.len() == 2 {
                            let symbol = id_parts[0];
                            let timeframe = match id_parts[1] {
                                "1MIN" => Timeframe::Minute1,
                                "5MINS" => Timeframe::Minute5,
                                "15MINS" => Timeframe::Minute15,
                                "30MINS" => Timeframe::Minute30,
                                "1HOUR" => Timeframe::Hour1,
                                "4HOURS" => Timeframe::Hour4,
                                "1DAY" => Timeframe::Day1,
                                _ => return None,
                            };
                            return Self::parse_candles(&candle_data, symbol, timeframe).map(WsMessage::Ohlcv);
                        }
                    }
                } else if channel == "v4_subaccounts" {
                    // Private channel - contains orders, positions, fills
                    if let Ok(subaccount_data) = serde_json::from_value::<DydxSubaccountData>(contents.clone()) {
                        // Process orders if present
                        if let Some(ref orders) = subaccount_data.orders {
                            if !orders.is_empty() {
                                return Self::parse_orders(orders).map(WsMessage::Order);
                            }
                        }
                        // Process fills if present
                        if let Some(ref fills) = subaccount_data.fills {
                            if !fills.is_empty() {
                                return Self::parse_fills(fills).map(WsMessage::MyTrade);
                            }
                        }
                        // Process positions if present
                        if let Some(ref positions) = subaccount_data.open_perpetual_positions {
                            if !positions.is_empty() {
                                return Self::parse_positions(positions).map(WsMessage::Position);
                            }
                        }
                    }
                }

                None
            }
            _ => None,
        }
    }

    /// Parse order updates from v4_subaccounts channel
    fn parse_orders(orders: &[DydxOrderUpdate]) -> Option<WsOrderEvent> {
        let parsed_orders: Vec<Order> = orders.iter().filter_map(|o| {
            let timestamp = o.updated_at.as_ref()
                .or(o.created_at.as_ref())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let symbol = Self::to_unified_symbol(&o.ticker);

            let status = match o.status.as_str() {
                "OPEN" | "PENDING" => OrderStatus::Open,
                "FILLED" => OrderStatus::Closed,
                "PARTIALLY_FILLED" | "BEST_EFFORT_OPENED" => OrderStatus::Open,
                "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
                "BEST_EFFORT_CANCELED" => OrderStatus::Canceled,
                _ => OrderStatus::Rejected,
            };

            let side = match o.side.as_str() {
                "BUY" => OrderSide::Buy,
                "SELL" => OrderSide::Sell,
                _ => return None, // Skip orders with unknown side
            };

            let order_type = match o.order_type.as_str() {
                "LIMIT" => OrderType::Limit,
                "MARKET" => OrderType::Market,
                "STOP_LIMIT" => OrderType::StopLimit,
                "STOP_MARKET" => OrderType::StopMarket,
                "TAKE_PROFIT" => OrderType::TakeProfit,
                "TAKE_PROFIT_MARKET" => OrderType::TakeProfitMarket,
                _ => OrderType::Limit,
            };

            let time_in_force = match o.time_in_force.as_deref() {
                Some("GTT") => Some(TimeInForce::GTT),
                Some("GTC") => Some(TimeInForce::GTC),
                Some("IOC") => Some(TimeInForce::IOC),
                Some("FOK") => Some(TimeInForce::FOK),
                _ => None,
            };

            let price = o.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
            let amount = Decimal::from_str(&o.size).ok()?;
            let filled = o.total_filled.as_ref().and_then(|f| Decimal::from_str(f).ok()).unwrap_or(Decimal::ZERO);
            let remaining = amount - filled;

            Some(Order {
                id: o.id.clone(),
                client_order_id: o.client_id.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                last_trade_timestamp: None,
                last_update_timestamp: Some(timestamp),
                status,
                symbol,
                order_type,
                time_in_force,
                side,
                price,
                average: None,
                amount,
                filled,
                remaining: Some(remaining),
                stop_price: o.trigger_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                trigger_price: o.trigger_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                take_profit_price: None,
                stop_loss_price: None,
                cost: None,
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                reduce_only: o.reduce_only,
                post_only: o.post_only,
                info: serde_json::to_value(o).unwrap_or_default(),
            })
        }).collect();

        // WsOrderEvent expects a single order, return the first one
        parsed_orders.into_iter().next().map(|order| WsOrderEvent { order })
    }

    /// Parse fill updates from v4_subaccounts channel
    fn parse_fills(fills: &[DydxFillUpdate]) -> Option<WsMyTradeEvent> {
        let trades: Vec<Trade> = fills.iter().filter_map(|f| {
            let timestamp = f.created_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let symbol = Self::to_unified_symbol(&f.ticker);
            let price = Decimal::from_str(&f.price).ok()?;
            let amount = Decimal::from_str(&f.size).ok()?;

            let side = match f.side.as_str() {
                "BUY" => Some(String::from("buy")),
                "SELL" => Some(String::from("sell")),
                _ => None,
            };

            let taker_or_maker = f.liquidity.as_ref().and_then(|l| match l.as_str() {
                "TAKER" => Some(TakerOrMaker::Taker),
                "MAKER" => Some(TakerOrMaker::Maker),
                _ => None,
            });

            Some(Trade {
                id: f.id.clone(),
                order: f.order_id.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol,
                trade_type: f.fill_type.clone(),
                side,
                taker_or_maker,
                price,
                amount,
                cost: Some(price * amount),
                fee: f.fee.as_ref().and_then(|fee| {
                    Decimal::from_str(fee).ok().map(|cost| crate::types::Fee {
                        cost: Some(cost),
                        currency: Some("USDC".to_string()),
                        rate: None,
                    })
                }),
                fees: Vec::new(),
                info: serde_json::to_value(f).unwrap_or_default(),
            })
        }).collect();

        if trades.is_empty() {
            return None;
        }

        Some(WsMyTradeEvent {
            symbol: trades.first().map(|t| t.symbol.clone()).unwrap_or_default(),
            trades,
        })
    }

    /// Parse position updates from v4_subaccounts channel
    fn parse_positions(positions: &HashMap<String, DydxPositionUpdate>) -> Option<WsPositionEvent> {
        let parsed_positions: Vec<Position> = positions.iter().filter_map(|(ticker, p)| {
            let symbol = Self::to_unified_symbol(ticker);
            let timestamp = p.updated_at.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let size = Decimal::from_str(&p.size).ok()?;
            let side = match p.side.as_str() {
                "LONG" => Some(PositionSide::Long),
                "SHORT" => Some(PositionSide::Short),
                _ => if size > Decimal::ZERO { Some(PositionSide::Long) } else { Some(PositionSide::Short) },
            };

            let entry_price = p.entry_price.as_ref().and_then(|p| Decimal::from_str(p).ok());
            let unrealized_pnl = p.unrealized_pnl.as_ref().and_then(|p| Decimal::from_str(p).ok());
            let realized_pnl = p.realized_pnl.as_ref().and_then(|p| Decimal::from_str(p).ok());

            Some(Position {
                symbol: symbol.clone(),
                id: Some(format!("{}:{}", symbol, p.subaccount_number)),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                contracts: Some(size.abs()),
                contract_size: Some(Decimal::ONE),
                side,
                notional: None,
                leverage: None,
                unrealized_pnl,
                realized_pnl,
                collateral: None,
                entry_price,
                mark_price: None,
                liquidation_price: None,
                margin_mode: Some(MarginMode::Cross),
                hedged: Some(false),
                maintenance_margin: None,
                maintenance_margin_percentage: None,
                initial_margin: None,
                initial_margin_percentage: None,
                margin_ratio: None,
                last_update_timestamp: Some(timestamp),
                last_price: None,
                stop_loss_price: None,
                take_profit_price: None,
                percentage: None,
                info: serde_json::to_value(p).unwrap_or_default(),
            })
        }).collect();

        if parsed_positions.is_empty() {
            return None;
        }

        Some(WsPositionEvent {
            positions: parsed_positions,
        })
    }

    /// Subscribe to a channel and return event stream
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        id: &str,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: self.get_ws_url().to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Build subscription message
        let subscribe_msg = DydxSubscribeMessage {
            msg_type: "subscribe".to_string(),
            channel: channel.to_string(),
            id: id.to_string(),
        };

        ws_client.send(&serde_json::to_string(&subscribe_msg).unwrap())?;

        self.ws_client = Some(ws_client);

        // Save subscription
        {
            let key = format!("{}:{}", channel, id);
            self.subscriptions.write().await.insert(key, channel.to_string());
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

    // ===== Private Channel Methods =====

    /// Subscribe to order updates (requires subaccount)
    ///
    /// Returns a stream of order status updates including new orders,
    /// fills, and cancellations.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ws = DydxWs::with_subaccount("dydx1abc...", 0, false);
    /// let mut rx = ws.watch_orders().await?;
    /// while let Some(msg) = rx.recv().await {
    ///     if let WsMessage::Order(event) = msg {
    ///         println!("Order update: {:?}", event.orders);
    ///     }
    /// }
    /// ```
    pub async fn watch_orders(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let subaccount_id = self.subaccount_id.as_ref()
            .ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
                message: "Subaccount ID required for private channels. Use DydxWs::with_subaccount()".to_string(),
            })?;

        let mut ws = DydxWs::with_subaccount(
            subaccount_id.split('/').next().unwrap_or(""),
            subaccount_id.split('/').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            self.testnet,
        );
        ws.subscribe_stream("v4_subaccounts", subaccount_id).await
    }

    /// Subscribe to position updates (requires subaccount)
    ///
    /// Returns a stream of position updates including size changes,
    /// PnL updates, and liquidations.
    pub async fn watch_positions(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let subaccount_id = self.subaccount_id.as_ref()
            .ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
                message: "Subaccount ID required for private channels. Use DydxWs::with_subaccount()".to_string(),
            })?;

        let mut ws = DydxWs::with_subaccount(
            subaccount_id.split('/').next().unwrap_or(""),
            subaccount_id.split('/').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            self.testnet,
        );
        ws.subscribe_stream("v4_subaccounts", subaccount_id).await
    }

    /// Subscribe to fill/trade updates (requires subaccount)
    ///
    /// Returns a stream of fill updates for your orders.
    pub async fn watch_my_trades(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let subaccount_id = self.subaccount_id.as_ref()
            .ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
                message: "Subaccount ID required for private channels. Use DydxWs::with_subaccount()".to_string(),
            })?;

        let mut ws = DydxWs::with_subaccount(
            subaccount_id.split('/').next().unwrap_or(""),
            subaccount_id.split('/').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            self.testnet,
        );
        ws.subscribe_stream("v4_subaccounts", subaccount_id).await
    }

    /// Subscribe to all private updates (orders, positions, fills)
    ///
    /// This is the most efficient way to get all account updates
    /// as they all come from the same v4_subaccounts channel.
    pub async fn watch_account(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let subaccount_id = self.subaccount_id.as_ref()
            .ok_or_else(|| crate::errors::CcxtError::AuthenticationError {
                message: "Subaccount ID required for private channels. Use DydxWs::with_subaccount()".to_string(),
            })?;

        let mut ws = DydxWs::with_subaccount(
            subaccount_id.split('/').next().unwrap_or(""),
            subaccount_id.split('/').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            self.testnet,
        );
        ws.subscribe_stream("v4_subaccounts", subaccount_id).await
    }

    /// Check if this instance has subaccount configured for private channels
    pub fn has_subaccount(&self) -> bool {
        self.subaccount_id.is_some()
    }

    /// Get the configured subaccount ID
    pub fn get_subaccount_id(&self) -> Option<&str> {
        self.subaccount_id.as_deref()
    }
}

#[async_trait]
impl WsExchange for DydxWs {
    /// Subscribe to ticker updates
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = if self.testnet { DydxWs::testnet() } else { DydxWs::new() };
        let formatted_symbol = Self::format_symbol(symbol);
        ws.subscribe_stream("v4_markets", &formatted_symbol).await
    }

    /// Subscribe to order book updates
    async fn watch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = if self.testnet { DydxWs::testnet() } else { DydxWs::new() };
        let formatted_symbol = Self::format_symbol(symbol);
        ws.subscribe_stream("v4_orderbook", &formatted_symbol).await
    }

    /// Subscribe to trade updates
    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = if self.testnet { DydxWs::testnet() } else { DydxWs::new() };
        let formatted_symbol = Self::format_symbol(symbol);
        ws.subscribe_stream("v4_trades", &formatted_symbol).await
    }

    /// Subscribe to OHLCV updates
    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = if self.testnet { DydxWs::testnet() } else { DydxWs::new() };
        let formatted_symbol = Self::format_symbol(symbol);

        // Convert timeframe to dYdX format
        let tf_str = match timeframe {
            Timeframe::Minute1 => "1MIN",
            Timeframe::Minute5 => "5MINS",
            Timeframe::Minute15 => "15MINS",
            Timeframe::Minute30 => "30MINS",
            Timeframe::Hour1 => "1HOUR",
            Timeframe::Hour4 => "4HOURS",
            Timeframe::Day1 => "1DAY",
            _ => return Err(crate::errors::CcxtError::NotSupported {
                feature: format!("Timeframe {:?}", timeframe),
            }),
        };

        let id = format!("{}/{}", formatted_symbol, tf_str);
        ws.subscribe_stream("v4_candles", &id).await
    }

    /// Connect to WebSocket
    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: self.get_ws_url().to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });

            ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    /// Close WebSocket connection
    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }

    /// Check if WebSocket is connected
    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

impl Default for DydxWs {
    fn default() -> Self {
        Self::new()
    }
}

// ===== dYdX WebSocket Message Structures =====

#[derive(Debug, Deserialize)]
struct DydxWsMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    contents: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct DydxSubscribeMessage {
    #[serde(rename = "type")]
    msg_type: String,
    channel: String,
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DydxMarketData {
    #[serde(default)]
    ticker: Option<String>,
    #[serde(default, rename = "oraclePrice")]
    oracle_price: Option<String>,
    #[serde(default, rename = "indexPrice")]
    index_price: Option<String>,
    #[serde(default, rename = "priceChange24H")]
    price_change_24h: Option<String>,
    #[serde(default, rename = "high24H")]
    high_24h: Option<String>,
    #[serde(default, rename = "low24H")]
    low_24h: Option<String>,
    #[serde(default, rename = "volume24H")]
    volume_24h: Option<String>,
    #[serde(default, rename = "trades24H")]
    trades_24h: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxOrderBookData {
    #[serde(default)]
    bids: Vec<DydxOrderBookLevel>,
    #[serde(default)]
    asks: Vec<DydxOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct DydxOrderBookLevel {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct DydxTradeData {
    #[serde(default)]
    trades: Vec<DydxTrade>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DydxTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    size: String,
    price: String,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxCandleData {
    #[serde(default)]
    candles: Vec<DydxCandle>,
}

#[derive(Debug, Deserialize)]
struct DydxCandle {
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    open: String,
    high: String,
    low: String,
    close: String,
    #[serde(rename = "baseTokenVolume")]
    base_token_volume: String,
}

// ===== Private Channel (v4_subaccounts) Structures =====

/// Subaccount data from v4_subaccounts channel
#[derive(Debug, Deserialize)]
struct DydxSubaccountData {
    /// Order updates
    #[serde(default)]
    orders: Option<Vec<DydxOrderUpdate>>,
    /// Fill updates
    #[serde(default)]
    fills: Option<Vec<DydxFillUpdate>>,
    /// Position updates (perpetual positions)
    #[serde(default, rename = "openPerpetualPositions")]
    open_perpetual_positions: Option<HashMap<String, DydxPositionUpdate>>,
    /// Asset positions (balances)
    #[serde(default, rename = "assetPositions")]
    asset_positions: Option<HashMap<String, DydxAssetPosition>>,
    /// Subaccount number
    #[serde(default, rename = "subaccountNumber")]
    subaccount_number: Option<u32>,
}

/// Order update from v4_subaccounts channel
#[derive(Debug, Serialize, Deserialize)]
struct DydxOrderUpdate {
    /// Order ID
    id: String,
    /// Client order ID
    #[serde(default, rename = "clientId")]
    client_id: Option<String>,
    /// Market ticker (e.g., "BTC-USD")
    ticker: String,
    /// Order side: "BUY" or "SELL"
    side: String,
    /// Order size
    size: String,
    /// Order price (for limit orders)
    #[serde(default)]
    price: Option<String>,
    /// Order type: "LIMIT", "MARKET", "STOP_LIMIT", etc.
    #[serde(rename = "type")]
    order_type: String,
    /// Order status: "OPEN", "FILLED", "CANCELED", etc.
    status: String,
    /// Time in force: "GTT", "IOC", "FOK"
    #[serde(default, rename = "timeInForce")]
    time_in_force: Option<String>,
    /// Reduce only flag
    #[serde(default, rename = "reduceOnly")]
    reduce_only: Option<bool>,
    /// Post only flag
    #[serde(default, rename = "postOnly")]
    post_only: Option<bool>,
    /// Total filled amount
    #[serde(default, rename = "totalFilled")]
    total_filled: Option<String>,
    /// Trigger price (for stop orders)
    #[serde(default, rename = "triggerPrice")]
    trigger_price: Option<String>,
    /// Good til block (short-term orders)
    #[serde(default, rename = "goodTilBlock")]
    good_til_block: Option<u32>,
    /// Good til block time (long-term orders)
    #[serde(default, rename = "goodTilBlockTime")]
    good_til_block_time: Option<String>,
    /// Created timestamp
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
    /// Updated timestamp
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<String>,
    /// Order flags
    #[serde(default, rename = "orderFlags")]
    order_flags: Option<String>,
}

/// Fill update from v4_subaccounts channel
#[derive(Debug, Serialize, Deserialize)]
struct DydxFillUpdate {
    /// Fill ID
    id: String,
    /// Order ID
    #[serde(default, rename = "orderId")]
    order_id: Option<String>,
    /// Market ticker
    ticker: String,
    /// Fill side: "BUY" or "SELL"
    side: String,
    /// Fill size
    size: String,
    /// Fill price
    price: String,
    /// Fill type: "LIMIT", "LIQUIDATED", etc.
    #[serde(default, rename = "type")]
    fill_type: Option<String>,
    /// Liquidity: "TAKER" or "MAKER"
    #[serde(default)]
    liquidity: Option<String>,
    /// Fee amount
    #[serde(default)]
    fee: Option<String>,
    /// Created timestamp
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

/// Position update from v4_subaccounts channel
#[derive(Debug, Serialize, Deserialize)]
struct DydxPositionUpdate {
    /// Position side: "LONG" or "SHORT"
    #[serde(default)]
    side: String,
    /// Position size
    size: String,
    /// Entry price
    #[serde(default, rename = "entryPrice")]
    entry_price: Option<String>,
    /// Unrealized PnL
    #[serde(default, rename = "unrealizedPnl")]
    unrealized_pnl: Option<String>,
    /// Realized PnL
    #[serde(default, rename = "realizedPnl")]
    realized_pnl: Option<String>,
    /// Sum of open orders
    #[serde(default, rename = "sumOpen")]
    sum_open: Option<String>,
    /// Sum of close orders
    #[serde(default, rename = "sumClose")]
    sum_close: Option<String>,
    /// Subaccount number
    #[serde(default, rename = "subaccountNumber")]
    subaccount_number: u32,
    /// Updated timestamp
    #[serde(default, rename = "updatedAt")]
    updated_at: Option<String>,
}

/// Asset position (balance) from v4_subaccounts channel
#[derive(Debug, Deserialize)]
struct DydxAssetPosition {
    /// Asset symbol (e.g., "USDC")
    #[serde(default)]
    symbol: Option<String>,
    /// Asset size (balance)
    #[serde(default)]
    size: Option<String>,
    /// Subaccount number
    #[serde(default, rename = "subaccountNumber")]
    subaccount_number: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(DydxWs::format_symbol("BTC/USD"), "BTC-USD");
        assert_eq!(DydxWs::format_symbol("ETH/USD"), "ETH-USD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DydxWs::to_unified_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(DydxWs::to_unified_symbol("ETH-USD"), "ETH/USD");
    }

    #[test]
    fn test_ws_client_creation() {
        let ws = DydxWs::new();
        assert!(ws.ws_client.is_none());
        assert!(!ws.testnet);
        assert!(ws.subaccount_id.is_none());
    }

    #[test]
    fn test_ws_testnet_creation() {
        let ws = DydxWs::testnet();
        assert!(ws.testnet);
        assert!(ws.subaccount_id.is_none());
    }

    #[test]
    fn test_ws_with_subaccount() {
        let ws = DydxWs::with_subaccount("dydx1abc123", 0, false);
        assert!(!ws.testnet);
        assert_eq!(ws.subaccount_id, Some("dydx1abc123/0".to_string()));
        assert!(ws.has_subaccount());
        assert_eq!(ws.get_subaccount_id(), Some("dydx1abc123/0"));
    }

    #[test]
    fn test_ws_with_subaccount_testnet() {
        let ws = DydxWs::with_subaccount("dydx1xyz789", 1, true);
        assert!(ws.testnet);
        assert_eq!(ws.subaccount_id, Some("dydx1xyz789/1".to_string()));
    }

    #[test]
    fn test_ws_url_mainnet() {
        let ws = DydxWs::new();
        assert_eq!(ws.get_ws_url(), WS_PUBLIC_URL);
    }

    #[test]
    fn test_ws_url_testnet() {
        let ws = DydxWs::testnet();
        assert_eq!(ws.get_ws_url(), WS_TESTNET_URL);
    }

    #[test]
    fn test_parse_order_update_json() {
        let json = r#"{
            "id": "order123",
            "ticker": "BTC-USD",
            "side": "BUY",
            "size": "0.1",
            "price": "50000",
            "type": "LIMIT",
            "status": "OPEN",
            "timeInForce": "GTT",
            "reduceOnly": false,
            "postOnly": true,
            "totalFilled": "0",
            "createdAt": "2024-01-01T00:00:00Z"
        }"#;

        let order: DydxOrderUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(order.id, "order123");
        assert_eq!(order.ticker, "BTC-USD");
        assert_eq!(order.side, "BUY");
        assert_eq!(order.order_type, "LIMIT");
        assert_eq!(order.status, "OPEN");
    }

    #[test]
    fn test_parse_fill_update_json() {
        let json = r#"{
            "id": "fill123",
            "orderId": "order123",
            "ticker": "ETH-USD",
            "side": "SELL",
            "size": "1.5",
            "price": "2000",
            "type": "LIMIT",
            "liquidity": "MAKER",
            "fee": "0.5"
        }"#;

        let fill: DydxFillUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(fill.id, "fill123");
        assert_eq!(fill.ticker, "ETH-USD");
        assert_eq!(fill.side, "SELL");
        assert_eq!(fill.fee, Some("0.5".to_string()));
    }

    #[test]
    fn test_parse_position_update_json() {
        let json = r#"{
            "side": "LONG",
            "size": "0.5",
            "entryPrice": "45000",
            "unrealizedPnl": "500",
            "realizedPnl": "100",
            "subaccountNumber": 0
        }"#;

        let position: DydxPositionUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(position.side, "LONG");
        assert_eq!(position.size, "0.5");
        assert_eq!(position.entry_price, Some("45000".to_string()));
        assert_eq!(position.subaccount_number, 0);
    }

    #[test]
    fn test_parse_subaccount_data() {
        let json = r#"{
            "orders": [{
                "id": "order1",
                "ticker": "BTC-USD",
                "side": "BUY",
                "size": "0.1",
                "type": "LIMIT",
                "status": "OPEN"
            }],
            "fills": [],
            "subaccountNumber": 0
        }"#;

        let data: DydxSubaccountData = serde_json::from_str(json).unwrap();
        assert!(data.orders.is_some());
        assert_eq!(data.orders.as_ref().unwrap().len(), 1);
        assert_eq!(data.subaccount_number, Some(0));
    }
}
