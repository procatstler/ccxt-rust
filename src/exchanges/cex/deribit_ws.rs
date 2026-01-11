//! Deribit WebSocket Implementation
//!
//! Deribit WebSocket API for real-time market data and private user data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use chrono::Utc;

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade,
    WsBalanceEvent, WsExchange, WsMessage, WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsPositionEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_BASE_URL: &str = "wss://www.deribit.com/ws/api/v2";
const WS_TESTNET_URL: &str = "wss://test.deribit.com/ws/api/v2";

/// Deribit WebSocket client
pub struct DeribitWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    request_id: Arc<RwLock<i64>>,
    testnet: bool,
    config: Option<ExchangeConfig>,
    access_token: Arc<RwLock<Option<String>>>,
}

impl DeribitWs {
    /// Create new Deribit WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            testnet: false,
            config: None,
            access_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Create new Deribit WebSocket client with testnet flag
    pub fn with_testnet(testnet: bool) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            testnet,
            config: None,
            access_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Create new Deribit WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        let testnet = config.is_sandbox();
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            request_id: Arc::new(RwLock::new(0)),
            testnet,
            config: Some(config),
            access_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Create with API credentials for private channels
    pub fn with_credentials(api_key: String, api_secret: String) -> Self {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        Self::with_config(config)
    }

    /// Set API credentials for private channels
    pub fn set_credentials(&mut self, api_key: String, api_secret: String) {
        let config = ExchangeConfig::default()
            .with_api_key(api_key)
            .with_api_secret(api_secret);
        self.config = Some(config);
    }

    /// Get next request ID
    async fn next_request_id(&self) -> i64 {
        let mut id = self.request_id.write().await;
        *id += 1;
        *id
    }

    /// Get WebSocket URL based on testnet flag
    fn get_url(&self) -> &'static str {
        if self.testnet {
            WS_TESTNET_URL
        } else {
            WS_BASE_URL
        }
    }

    /// Timeframe to Deribit interval mapping
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1",
            Timeframe::Minute3 => "3",
            Timeframe::Minute5 => "5",
            Timeframe::Minute15 => "15",
            Timeframe::Minute30 => "30",
            Timeframe::Hour1 => "60",
            Timeframe::Hour2 => "120",
            Timeframe::Hour4 => "240",
            Timeframe::Hour6 => "360",
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "1D",
            _ => "60", // Default to 1 hour
        }
    }

    /// Parse unified symbol from Deribit instrument name
    fn to_unified_symbol(instrument_name: &str) -> String {
        // Deribit format: BTC-PERPETUAL, BTC-25DEC20, BTC-25DEC20-36000-C
        let parts: Vec<&str> = instrument_name.split('-').collect();
        if parts.is_empty() {
            return instrument_name.to_string();
        }

        let base = parts[0];

        if parts.len() == 2 && parts[1] == "PERPETUAL" {
            return format!("{base}/USD:{base}");
        }

        if parts.len() == 2 {
            // Future: BTC-25DEC20
            return format!("{base}/USD:{base}");
        }

        if parts.len() == 4 {
            // Option: BTC-25DEC20-36000-C
            let strike = parts[2];
            let option_type = parts[3];
            return format!("{base}/USD:{base}:{strike}:{option_type}");
        }

        instrument_name.to_string()
    }

    /// Parse ticker message
    fn parse_ticker(data: &DeribitTickerData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.instrument_name);
        let timestamp = data.timestamp;

        let (high, low, volume, percentage) = if let Some(ref stats) = data.stats {
            (
                stats.high.and_then(Decimal::from_f64_retain),
                stats.low.and_then(Decimal::from_f64_retain),
                stats.volume.and_then(Decimal::from_f64_retain),
                stats.price_change.and_then(Decimal::from_f64_retain),
            )
        } else {
            (None, None, None, None)
        };

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high,
            low,
            bid: data
                .best_bid_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid_volume: data
                .best_bid_amount
                .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
            ask: data
                .best_ask_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            ask_volume: data
                .best_ask_amount
                .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
            vwap: None,
            open: None,
            close: data
                .last_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            last: data
                .last_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            previous_close: None,
            change: None,
            percentage,
            average: None,
            base_volume: volume,
            quote_volume: None,
            index_price: data
                .index_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            mark_price: data
                .mark_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(data: &DeribitOrderBookData, symbol: &str) -> WsOrderBookEvent {
        let is_snapshot = data.r#type.as_deref() == Some("snapshot");

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 3 {
                    // Format: ["new"|"change"|"delete", price, amount]
                    Some(OrderBookEntry {
                        price: b[1].as_f64().and_then(Decimal::from_f64_retain)?,
                        amount: b[2].as_f64().and_then(Decimal::from_f64_retain)?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 3 {
                    Some(OrderBookEntry {
                        price: a[1].as_f64().and_then(Decimal::from_f64_retain)?,
                        amount: a[2].as_f64().and_then(Decimal::from_f64_retain)?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: data.change_id,
            bids,
            asks,
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// Parse trade message
    fn parse_trade(trade_data: &DeribitTradeData, symbol: &str) -> Trade {
        let timestamp = trade_data.timestamp;

        let side = match trade_data.direction.as_str() {
            "buy" => Some("buy".to_string()),
            "sell" => Some("sell".to_string()),
            _ => None,
        };

        let price = Decimal::from_f64_retain(trade_data.price).unwrap_or_default();
        let amount = Decimal::from_f64_retain(trade_data.amount).unwrap_or_default();

        Trade {
            id: trade_data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(trade_data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV message
    fn parse_ohlcv(data: &DeribitOhlcvData, _timeframe: Timeframe) -> OHLCV {
        OHLCV {
            timestamp: data.tick,
            open: Decimal::from_f64_retain(data.open).unwrap_or_default(),
            high: Decimal::from_f64_retain(data.high).unwrap_or_default(),
            low: Decimal::from_f64_retain(data.low).unwrap_or_default(),
            close: Decimal::from_f64_retain(data.close).unwrap_or_default(),
            volume: Decimal::from_f64_retain(data.volume).unwrap_or_default(),
        }
    }

    // === Private Channel Parsing Methods ===

    /// Parse order update from WebSocket
    fn parse_order_update(data: &DeribitWsOrderData) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.instrument_name);
        let timestamp = data
            .last_update_timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = match data.direction.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("market") => OrderType::Market,
            Some("limit") => OrderType::Limit,
            Some("stop_market") => OrderType::StopLoss,
            Some("stop_limit") => OrderType::StopLossLimit,
            Some("take_market") => OrderType::TakeProfit,
            Some("take_limit") => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        };

        let status = match data.order_state.as_deref() {
            Some("open") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("rejected") => OrderStatus::Rejected,
            Some("cancelled") => OrderStatus::Canceled,
            Some("untriggered") => OrderStatus::Open,
            _ => OrderStatus::Open,
        };

        let price = data.price.and_then(Decimal::from_f64_retain);
        let amount = data
            .amount
            .and_then(Decimal::from_f64_retain)
            .unwrap_or_default();
        let filled = data
            .filled_amount
            .and_then(Decimal::from_f64_retain)
            .unwrap_or_default();
        let remaining = Some(amount - filled);
        let average = data.average_price.and_then(Decimal::from_f64_retain);
        let stop_price = data.stop_price.and_then(Decimal::from_f64_retain);
        let trigger_price = data.trigger_price.and_then(Decimal::from_f64_retain);

        let order = Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.label.clone(),
            timestamp: Some(data.creation_timestamp.unwrap_or(timestamp)),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(
                    data.creation_timestamp.unwrap_or(timestamp),
                )
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(timestamp),
            status,
            symbol,
            order_type,
            time_in_force: data
                .time_in_force
                .as_ref()
                .and_then(|tif| match tif.as_str() {
                    "GTC" | "good_til_cancelled" => Some(TimeInForce::GTC),
                    "IOC" | "immediate_or_cancel" => Some(TimeInForce::IOC),
                    "FOK" | "fill_or_kill" => Some(TimeInForce::FOK),
                    _ => None,
                }),
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price,
            trigger_price,
            stop_loss_price: None,
            take_profit_price: None,
            cost: average.map(|a| a * filled),
            trades: Vec::new(),
            reduce_only: data.reduce_only,
            post_only: data.post_only,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// Parse my trade (user trade) from WebSocket
    fn parse_my_trade(data: &DeribitWsUserTradeData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.instrument_name);
        let timestamp = data.timestamp;

        let side = match data.direction.as_str() {
            "buy" => Some("buy".to_string()),
            "sell" => Some("sell".to_string()),
            _ => None,
        };

        let price = Decimal::from_f64_retain(data.price).unwrap_or_default();
        let amount = Decimal::from_f64_retain(data.amount).unwrap_or_default();

        let fee = data.fee.and_then(Decimal::from_f64_retain).map(|f| Fee {
            currency: data.fee_currency.clone(),
            cost: Some(f.abs()),
            rate: None,
        });

        let trade = Trade {
            id: data.trade_id.clone(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker: data.liquidity.as_ref().map(|l| {
                if l == "M" {
                    TakerOrMaker::Maker
                } else {
                    TakerOrMaker::Taker
                }
            }),
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// Parse position update from WebSocket
    fn parse_position_update(positions: &[DeribitWsPositionData]) -> WsPositionEvent {
        let timestamp = Utc::now().timestamp_millis();
        let parsed_positions: Vec<Position> = positions
            .iter()
            .map(|pos| {
                let symbol = Self::to_unified_symbol(&pos.instrument_name);
                let size = pos.size.unwrap_or(0.0);
                let side = if size > 0.0 {
                    Some(PositionSide::Long)
                } else if size < 0.0 {
                    Some(PositionSide::Short)
                } else {
                    None
                };

                Position {
                    id: None,
                    symbol,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    contracts: pos.size.and_then(|s| Decimal::from_f64_retain(s.abs())),
                    contract_size: None,
                    side,
                    notional: None,
                    leverage: pos
                        .leverage
                        .and_then(|l| Decimal::from_f64_retain(l as f64)),
                    collateral: None,
                    initial_margin: pos.initial_margin.and_then(Decimal::from_f64_retain),
                    initial_margin_percentage: None,
                    maintenance_margin: pos.maintenance_margin.and_then(Decimal::from_f64_retain),
                    maintenance_margin_percentage: None,
                    entry_price: pos.average_price.and_then(Decimal::from_f64_retain),
                    mark_price: pos.mark_price.and_then(Decimal::from_f64_retain),
                    liquidation_price: pos
                        .estimated_liquidation_price
                        .and_then(Decimal::from_f64_retain),
                    unrealized_pnl: pos.floating_profit_loss.and_then(Decimal::from_f64_retain),
                    realized_pnl: pos.realized_profit_loss.and_then(Decimal::from_f64_retain),
                    last_price: None,
                    last_update_timestamp: Some(timestamp),
                    margin_ratio: None,
                    margin_mode: Some(MarginMode::Cross), // Deribit uses cross margin by default
                    hedged: Some(false),
                    stop_loss_price: None,
                    take_profit_price: None,
                    percentage: None,
                    info: serde_json::to_value(pos).unwrap_or_default(),
                }
            })
            .collect();

        WsPositionEvent {
            positions: parsed_positions,
        }
    }

    /// Parse account/portfolio update from WebSocket
    fn parse_portfolio_update(data: &DeribitWsPortfolioData) -> WsBalanceEvent {
        let timestamp = Utc::now().timestamp_millis();
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        let currency = data.currency.clone();
        let balance = Balance {
            free: data.available_funds.and_then(Decimal::from_f64_retain),
            used: data.maintenance_margin.and_then(Decimal::from_f64_retain),
            total: data.balance.and_then(Decimal::from_f64_retain),
            debt: None,
        };
        currencies.insert(currency, balance);

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsBalanceEvent { balances }
    }

    /// Authenticate WebSocket connection
    async fn authenticate(&mut self) -> CcxtResult<()> {
        let config = self.config.as_ref().ok_or(CcxtError::AuthenticationError {
            message: "API key and secret required for authentication".into(),
        })?;

        let api_key = config.api_key().ok_or(CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = config.api_secret().ok_or(CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let request_id = self.next_request_id().await;
        let auth_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "public/auth",
            "params": {
                "grant_type": "client_credentials",
                "client_id": api_key,
                "client_secret": api_secret
            }
        });

        if let Some(client) = &self.ws_client {
            let msg_str = serde_json::to_string(&auth_msg).map_err(|e| CcxtError::ParseError {
                data_type: "AuthMessage".to_string(),
                message: e.to_string(),
            })?;
            client.send(&msg_str)?;
        }

        Ok(())
    }

    /// Subscribe to private channel and return event receiver
    async fn subscribe_private_channel(
        &mut self,
        channels: Vec<String>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = self.get_url();

        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Authenticate first
        self.authenticate().await?;

        // Wait for authentication response (in a real implementation, you'd wait for the auth success message)
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Send subscription request to private channels
        let request_id = self.next_request_id().await;
        let subscribe_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "private/subscribe",
            "params": {
                "channels": channels
            }
        });

        if let Some(client) = &self.ws_client {
            let msg_str =
                serde_json::to_string(&subscribe_msg).map_err(|e| CcxtError::ParseError {
                    data_type: "SubscribeMessage".to_string(),
                    message: e.to_string(),
                })?;
            client.send(&msg_str)?;
        }

        // Store subscriptions
        for channel in channels {
            self.subscriptions
                .write()
                .await
                .insert(channel.clone(), channel);
        }

        // Event processing task
        let tx = event_tx.clone();
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
                        if let Some(ws_msg) = Self::process_message(&msg) {
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

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: DeribitMessage = match serde_json::from_str(msg) {
            Ok(m) => m,
            Err(_) => return None,
        };

        // Check for errors
        if let Some(error) = json.error {
            return Some(WsMessage::Error(format!(
                "{}: {}",
                error.code, error.message
            )));
        }

        // Handle subscription notifications
        if json.method.as_deref() == Some("subscription") {
            if let Some(params) = json.params {
                if let Some(channel) = params.channel {
                    let parts: Vec<&str> = channel.split('.').collect();
                    let channel_type = parts.first()?;

                    match *channel_type {
                        "ticker" => {
                            if let Ok(data) =
                                serde_json::from_value::<DeribitTickerData>(params.data)
                            {
                                return Some(WsMessage::Ticker(Self::parse_ticker(&data)));
                            }
                        },
                        "book" => {
                            if let Ok(data) =
                                serde_json::from_value::<DeribitOrderBookData>(params.data)
                            {
                                let symbol = Self::to_unified_symbol(&data.instrument_name);
                                return Some(WsMessage::OrderBook(Self::parse_order_book(
                                    &data, &symbol,
                                )));
                            }
                        },
                        "trades" => {
                            if let Ok(trades) =
                                serde_json::from_value::<Vec<DeribitTradeData>>(params.data)
                            {
                                if let Some(first_trade) = trades.first() {
                                    let symbol =
                                        Self::to_unified_symbol(&first_trade.instrument_name);
                                    let parsed_trades: Vec<Trade> = trades
                                        .iter()
                                        .map(|t| Self::parse_trade(t, &symbol))
                                        .collect();
                                    return Some(WsMessage::Trade(WsTradeEvent {
                                        symbol,
                                        trades: parsed_trades,
                                    }));
                                }
                            }
                        },
                        "chart" => {
                            if let Ok(data) =
                                serde_json::from_value::<DeribitOhlcvData>(params.data)
                            {
                                // Extract timeframe from channel (e.g., "chart.trades.BTC-PERPETUAL.1")
                                let timeframe = if parts.len() >= 4 {
                                    match parts[3] {
                                        "1" => Timeframe::Minute1,
                                        "3" => Timeframe::Minute3,
                                        "5" => Timeframe::Minute5,
                                        "15" => Timeframe::Minute15,
                                        "30" => Timeframe::Minute30,
                                        "60" => Timeframe::Hour1,
                                        "120" => Timeframe::Hour2,
                                        "240" => Timeframe::Hour4,
                                        "360" => Timeframe::Hour6,
                                        "720" => Timeframe::Hour12,
                                        "1D" => Timeframe::Day1,
                                        _ => Timeframe::Minute1,
                                    }
                                } else {
                                    Timeframe::Minute1
                                };

                                if parts.len() >= 3 {
                                    let symbol = Self::to_unified_symbol(parts[2]);
                                    return Some(WsMessage::Ohlcv(WsOhlcvEvent {
                                        symbol,
                                        timeframe,
                                        ohlcv: Self::parse_ohlcv(&data, timeframe),
                                    }));
                                }
                            }
                        },
                        // === Private Channels ===
                        "user" => {
                            // user.orders.{instrument_name} - order updates
                            // user.trades.{instrument_name} - my trades
                            // user.portfolio.{currency} - portfolio/balance updates
                            // user.changes.{instrument_name} - position changes
                            if parts.len() >= 2 {
                                match parts[1] {
                                    "orders" => {
                                        if let Ok(orders) =
                                            serde_json::from_value::<Vec<DeribitWsOrderData>>(
                                                params.data.clone(),
                                            )
                                        {
                                            if let Some(order) = orders.first() {
                                                return Some(WsMessage::Order(
                                                    Self::parse_order_update(order),
                                                ));
                                            }
                                        }
                                        // Single order format
                                        if let Ok(order) =
                                            serde_json::from_value::<DeribitWsOrderData>(
                                                params.data,
                                            )
                                        {
                                            return Some(WsMessage::Order(
                                                Self::parse_order_update(&order),
                                            ));
                                        }
                                    },
                                    "trades" => {
                                        if let Ok(trades) =
                                            serde_json::from_value::<Vec<DeribitWsUserTradeData>>(
                                                params.data,
                                            )
                                        {
                                            if let Some(trade) = trades.first() {
                                                return Some(WsMessage::MyTrade(
                                                    Self::parse_my_trade(trade),
                                                ));
                                            }
                                        }
                                    },
                                    "portfolio" => {
                                        if let Ok(data) =
                                            serde_json::from_value::<DeribitWsPortfolioData>(
                                                params.data,
                                            )
                                        {
                                            return Some(WsMessage::Balance(
                                                Self::parse_portfolio_update(&data),
                                            ));
                                        }
                                    },
                                    "changes" => {
                                        // Position changes
                                        if let Ok(changes) =
                                            serde_json::from_value::<DeribitWsChangesData>(
                                                params.data,
                                            )
                                        {
                                            if !changes.positions.is_empty() {
                                                return Some(WsMessage::Position(
                                                    Self::parse_position_update(&changes.positions),
                                                ));
                                            }
                                        }
                                    },
                                    _ => {},
                                }
                            }
                        },
                        _ => {},
                    }
                }
            }
        }

        // Handle authentication response
        if let Some(result) = json.result {
            if result.get("access_token").is_some() {
                return Some(WsMessage::Authenticated);
            }
        }

        None
    }

    /// Subscribe to channel and return event receiver
    async fn subscribe_channel(
        &mut self,
        channels: Vec<String>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let url = self.get_url();

        let mut ws_client = WsClient::new(WsConfig {
            url: url.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // Send subscription request
        let request_id = self.next_request_id().await;
        let subscribe_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "public/subscribe",
            "params": {
                "channels": channels
            }
        });

        if let Some(client) = &self.ws_client {
            let msg_str =
                serde_json::to_string(&subscribe_msg).map_err(|e| CcxtError::ParseError {
                    data_type: "SubscribeMessage".to_string(),
                    message: e.to_string(),
                })?;
            client.send(&msg_str)?;
        }

        // Store subscriptions
        for channel in channels {
            self.subscriptions
                .write()
                .await
                .insert(channel.clone(), channel);
        }

        // Event processing task
        let tx = event_tx.clone();
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
                        if let Some(ws_msg) = Self::process_message(&msg) {
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

impl Default for DeribitWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for DeribitWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        // Convert symbol to Deribit instrument name (e.g., BTC/USD:BTC -> BTC-PERPETUAL)
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let channel = format!("ticker.{instrument}.100ms");
        client.subscribe_channel(vec![channel]).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let channels: Vec<String> = symbols
            .iter()
            .map(|s| {
                let instrument = s.replace("/USD:", "-").replace("/", "-");
                format!("ticker.{instrument}.100ms")
            })
            .collect();
        client.subscribe_channel(channels).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let depth = limit.unwrap_or(20);
        // Format: book.{instrument}.{group}.{depth}.{interval}
        // Using "none" for group, depth for limit, "100ms" for interval
        let channel = format!("book.{instrument}.none.{depth}.100ms");
        client.subscribe_channel(vec![channel]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let channel = format!("trades.{instrument}.100ms");
        client.subscribe_channel(vec![channel]).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let instrument = symbol.replace("/USD:", "-").replace("/", "-");
        let interval = Self::format_interval(timeframe);
        let channel = format!("chart.trades.{instrument}.{interval}");
        client.subscribe_channel(vec![channel]).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Deribit connects on subscription
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

    // === Private Channel Methods ===

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        self.authenticate().await
    }

    async fn watch_orders(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        let channel = if let Some(sym) = symbol {
            let instrument = sym.replace("/USD:", "-").replace("/", "-");
            format!("user.orders.{instrument}.raw")
        } else {
            "user.orders.any.raw".to_string()
        };

        client.subscribe_private_channel(vec![channel]).await
    }

    async fn watch_my_trades(
        &self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        let channel = if let Some(sym) = symbol {
            let instrument = sym.replace("/USD:", "-").replace("/", "-");
            format!("user.trades.{instrument}.raw")
        } else {
            "user.trades.any.raw".to_string()
        };

        client.subscribe_private_channel(vec![channel]).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        // Subscribe to both BTC and ETH portfolio updates
        let channels = vec![
            "user.portfolio.btc".to_string(),
            "user.portfolio.eth".to_string(),
        ];

        client.subscribe_private_channel(channels).await
    }

    async fn watch_positions(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };

        let channels: Vec<String> = if let Some(syms) = symbols {
            syms.iter()
                .map(|s| {
                    let instrument = s.replace("/USD:", "-").replace("/", "-");
                    format!("user.changes.{instrument}.raw")
                })
                .collect()
        } else {
            vec!["user.changes.any.raw".to_string()]
        };

        client.subscribe_private_channel(channels).await
    }
}

// === Deribit WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct DeribitMessage {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<i64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<DeribitParams>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<DeribitError>,
}

#[derive(Debug, Deserialize)]
struct DeribitError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct DeribitParams {
    #[serde(default)]
    channel: Option<String>,
    data: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitTickerData {
    instrument_name: String,
    timestamp: i64,
    #[serde(default)]
    best_bid_price: Option<f64>,
    #[serde(default)]
    best_bid_amount: Option<f64>,
    #[serde(default)]
    best_ask_price: Option<f64>,
    #[serde(default)]
    best_ask_amount: Option<f64>,
    #[serde(default)]
    last_price: Option<f64>,
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    index_price: Option<f64>,
    #[serde(default)]
    stats: Option<DeribitStats>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitStats {
    #[serde(default)]
    high: Option<f64>,
    #[serde(default)]
    low: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    price_change: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOrderBookData {
    r#type: Option<String>, // "snapshot" or "change"
    instrument_name: String,
    timestamp: i64,
    #[serde(default)]
    change_id: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<serde_json::Value>>, // [["new"|"change"|"delete", price, amount]]
    #[serde(default)]
    asks: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitTradeData {
    trade_id: String,
    instrument_name: String,
    timestamp: i64,
    direction: String, // "buy" or "sell"
    price: f64,
    amount: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOhlcvData {
    tick: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
}

// === Private Channel Data Types ===

#[derive(Debug, Deserialize, Serialize, Clone)]
struct DeribitWsOrderData {
    #[serde(default)]
    order_id: Option<String>,
    instrument_name: String,
    #[serde(default)]
    order_state: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    price: Option<f64>,
    #[serde(default)]
    amount: Option<f64>,
    #[serde(default)]
    filled_amount: Option<f64>,
    #[serde(default)]
    average_price: Option<f64>,
    #[serde(default)]
    creation_timestamp: Option<i64>,
    #[serde(default)]
    last_update_timestamp: Option<i64>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    trigger_price: Option<f64>,
    #[serde(default)]
    stop_price: Option<f64>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    post_only: Option<bool>,
    #[serde(default)]
    profit_loss: Option<f64>,
    #[serde(default)]
    commission: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct DeribitWsUserTradeData {
    trade_id: String,
    instrument_name: String,
    timestamp: i64,
    direction: String,
    price: f64,
    amount: f64,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    fee: Option<f64>,
    #[serde(default)]
    fee_currency: Option<String>,
    #[serde(default)]
    liquidity: Option<String>, // "M" for maker, "T" for taker
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    index_price: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct DeribitWsPositionData {
    instrument_name: String,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    size: Option<f64>,
    #[serde(default)]
    average_price: Option<f64>,
    #[serde(default)]
    floating_profit_loss: Option<f64>,
    #[serde(default)]
    realized_profit_loss: Option<f64>,
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    initial_margin: Option<f64>,
    #[serde(default)]
    maintenance_margin: Option<f64>,
    #[serde(default)]
    leverage: Option<i32>,
    #[serde(default)]
    estimated_liquidation_price: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct DeribitWsPortfolioData {
    currency: String,
    #[serde(default)]
    balance: Option<f64>,
    #[serde(default)]
    equity: Option<f64>,
    #[serde(default)]
    available_funds: Option<f64>,
    #[serde(default)]
    available_withdrawal_funds: Option<f64>,
    #[serde(default)]
    initial_margin: Option<f64>,
    #[serde(default)]
    maintenance_margin: Option<f64>,
    #[serde(default)]
    margin_balance: Option<f64>,
    #[serde(default)]
    total_pl: Option<f64>,
    #[serde(default)]
    session_upl: Option<f64>,
    #[serde(default)]
    session_rpl: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct DeribitWsChangesData {
    instrument_name: String,
    #[serde(default)]
    positions: Vec<DeribitWsPositionData>,
    #[serde(default)]
    orders: Vec<DeribitWsOrderData>,
    #[serde(default)]
    trades: Vec<DeribitWsUserTradeData>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DeribitWs::to_unified_symbol("BTC-PERPETUAL"), "BTC/USD:BTC");
        assert_eq!(DeribitWs::to_unified_symbol("ETH-PERPETUAL"), "ETH/USD:ETH");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(DeribitWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(DeribitWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(DeribitWs::format_interval(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_deribit_ws_creation() {
        let ws = DeribitWs::new();
        assert!(!ws.testnet);

        let ws_testnet = DeribitWs::with_testnet(true);
        assert!(ws_testnet.testnet);
    }

    // === Private Channel Tests ===

    #[test]
    fn test_parse_order_update() {
        let data = DeribitWsOrderData {
            order_id: Some("order-123".to_string()),
            instrument_name: "BTC-PERPETUAL".to_string(),
            order_state: Some("open".to_string()),
            direction: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            time_in_force: Some("GTC".to_string()),
            price: Some(50000.0),
            amount: Some(100.0),
            filled_amount: Some(0.0),
            average_price: None,
            creation_timestamp: Some(1705315800000),
            last_update_timestamp: Some(1705315800000),
            label: Some("test-label".to_string()),
            trigger_price: None,
            stop_price: None,
            reduce_only: Some(false),
            post_only: Some(true),
            profit_loss: None,
            commission: None,
        };

        let event = DeribitWs::parse_order_update(&data);
        assert_eq!(event.order.id, "order-123");
        assert_eq!(event.order.symbol, "BTC/USD:BTC");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.amount, Decimal::new(100, 0));
        assert_eq!(event.order.post_only, Some(true));
    }

    #[test]
    fn test_parse_order_update_filled() {
        let data = DeribitWsOrderData {
            order_id: Some("order-456".to_string()),
            instrument_name: "ETH-PERPETUAL".to_string(),
            order_state: Some("filled".to_string()),
            direction: Some("sell".to_string()),
            order_type: Some("market".to_string()),
            time_in_force: None,
            price: None,
            amount: Some(50.0),
            filled_amount: Some(50.0),
            average_price: Some(3000.0),
            creation_timestamp: Some(1705315800000),
            last_update_timestamp: Some(1705315900000),
            label: None,
            trigger_price: None,
            stop_price: None,
            reduce_only: None,
            post_only: None,
            profit_loss: Some(100.0),
            commission: Some(0.5),
        };

        let event = DeribitWs::parse_order_update(&data);
        assert_eq!(event.order.status, OrderStatus::Closed);
        assert_eq!(event.order.side, OrderSide::Sell);
        assert_eq!(event.order.order_type, OrderType::Market);
        assert_eq!(event.order.filled, Decimal::new(50, 0));
    }

    #[test]
    fn test_parse_my_trade() {
        let data = DeribitWsUserTradeData {
            trade_id: "trade-789".to_string(),
            instrument_name: "BTC-PERPETUAL".to_string(),
            timestamp: 1705315800000,
            direction: "buy".to_string(),
            price: 50000.0,
            amount: 10.0,
            order_id: Some("order-123".to_string()),
            fee: Some(0.00075),
            fee_currency: Some("BTC".to_string()),
            liquidity: Some("M".to_string()),
            mark_price: Some(50001.0),
            index_price: Some(49999.0),
        };

        let event = DeribitWs::parse_my_trade(&data);
        assert_eq!(event.symbol, "BTC/USD:BTC");
        assert_eq!(event.trades.len(), 1);

        let trade = &event.trades[0];
        assert_eq!(trade.id, "trade-789");
        assert_eq!(trade.order, Some("order-123".to_string()));
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.price, Decimal::new(50000, 0));
        assert_eq!(trade.amount, Decimal::new(10, 0));
        assert_eq!(trade.taker_or_maker, Some(TakerOrMaker::Maker));
        assert!(trade.fee.is_some());
    }

    #[test]
    fn test_parse_position_update() {
        let positions = vec![DeribitWsPositionData {
            instrument_name: "BTC-PERPETUAL".to_string(),
            direction: Some("buy".to_string()),
            size: Some(1000.0),
            average_price: Some(50000.0),
            floating_profit_loss: Some(500.0),
            realized_profit_loss: Some(100.0),
            mark_price: Some(50500.0),
            initial_margin: Some(0.1),
            maintenance_margin: Some(0.05),
            leverage: Some(10),
            estimated_liquidation_price: Some(40000.0),
        }];

        let event = DeribitWs::parse_position_update(&positions);
        assert_eq!(event.positions.len(), 1);

        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "BTC/USD:BTC");
        assert_eq!(pos.side, Some(PositionSide::Long));
        assert_eq!(pos.contracts, Some(Decimal::new(1000, 0)));
        assert_eq!(pos.leverage, Some(Decimal::new(10, 0)));
    }

    #[test]
    fn test_parse_position_update_short() {
        let positions = vec![DeribitWsPositionData {
            instrument_name: "ETH-PERPETUAL".to_string(),
            direction: Some("sell".to_string()),
            size: Some(-500.0),
            average_price: Some(3000.0),
            floating_profit_loss: Some(250.0),
            realized_profit_loss: None,
            mark_price: Some(2950.0),
            initial_margin: None,
            maintenance_margin: None,
            leverage: Some(5),
            estimated_liquidation_price: Some(3500.0),
        }];

        let event = DeribitWs::parse_position_update(&positions);
        assert_eq!(event.positions.len(), 1);

        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "ETH/USD:ETH");
        assert_eq!(pos.side, Some(PositionSide::Short));
        assert_eq!(pos.contracts, Some(Decimal::new(500, 0)));
    }

    #[test]
    fn test_parse_portfolio_update() {
        let data = DeribitWsPortfolioData {
            currency: "BTC".to_string(),
            balance: Some(1.5),
            equity: Some(1.6),
            available_funds: Some(1.2),
            available_withdrawal_funds: Some(1.0),
            initial_margin: Some(0.2),
            maintenance_margin: Some(0.1),
            margin_balance: Some(1.5),
            total_pl: Some(0.5),
            session_upl: Some(0.1),
            session_rpl: Some(0.05),
        };

        let event = DeribitWs::parse_portfolio_update(&data);
        let btc_balance = event.balances.currencies.get("BTC");
        assert!(btc_balance.is_some());

        let balance = btc_balance.unwrap();
        assert_eq!(balance.free, Decimal::from_f64_retain(1.2));
        assert_eq!(balance.total, Decimal::from_f64_retain(1.5));
    }

    #[test]
    fn test_process_message_order() {
        let msg = r#"{"jsonrpc":"2.0","method":"subscription","params":{"channel":"user.orders.BTC-PERPETUAL.raw","data":[{"order_id":"order-123","instrument_name":"BTC-PERPETUAL","order_state":"open","direction":"buy","order_type":"limit","price":50000.0,"amount":100.0,"filled_amount":0.0}]}}"#;

        let result = DeribitWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "order-123");
            assert_eq!(event.order.symbol, "BTC/USD:BTC");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_my_trade() {
        let msg = r#"{"jsonrpc":"2.0","method":"subscription","params":{"channel":"user.trades.BTC-PERPETUAL.raw","data":[{"trade_id":"trade-456","instrument_name":"BTC-PERPETUAL","timestamp":1705315800000,"direction":"buy","price":50000.0,"amount":10.0}]}}"#;

        let result = DeribitWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "BTC/USD:BTC");
            assert!(!event.trades.is_empty());
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_message_portfolio() {
        let msg = r#"{"jsonrpc":"2.0","method":"subscription","params":{"channel":"user.portfolio.btc","data":{"currency":"BTC","balance":1.5,"available_funds":1.2}}}"#;

        let result = DeribitWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_message_auth_success() {
        let msg = r#"{"jsonrpc":"2.0","id":1,"result":{"access_token":"test_token","refresh_token":"refresh_token","expires_in":900}}"#;

        let result = DeribitWs::process_message(msg);
        assert!(matches!(result, Some(WsMessage::Authenticated)));
    }

    #[test]
    fn test_order_status_parsing() {
        let test_cases = vec![
            ("open", OrderStatus::Open),
            ("filled", OrderStatus::Closed),
            ("rejected", OrderStatus::Rejected),
            ("cancelled", OrderStatus::Canceled),
            ("untriggered", OrderStatus::Open),
        ];

        for (status_str, expected) in test_cases {
            let data = DeribitWsOrderData {
                order_id: Some("test".to_string()),
                instrument_name: "BTC-PERPETUAL".to_string(),
                order_state: Some(status_str.to_string()),
                direction: None,
                order_type: None,
                time_in_force: None,
                price: None,
                amount: None,
                filled_amount: None,
                average_price: None,
                creation_timestamp: None,
                last_update_timestamp: None,
                label: None,
                trigger_price: None,
                stop_price: None,
                reduce_only: None,
                post_only: None,
                profit_loss: None,
                commission: None,
            };

            let event = DeribitWs::parse_order_update(&data);
            assert_eq!(
                event.order.status, expected,
                "Status {status_str} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn test_order_type_parsing() {
        let test_cases = vec![
            ("market", OrderType::Market),
            ("limit", OrderType::Limit),
            ("stop_market", OrderType::StopLoss),
            ("stop_limit", OrderType::StopLossLimit),
            ("take_market", OrderType::TakeProfit),
            ("take_limit", OrderType::TakeProfitLimit),
        ];

        for (type_str, expected) in test_cases {
            let data = DeribitWsOrderData {
                order_id: Some("test".to_string()),
                instrument_name: "BTC-PERPETUAL".to_string(),
                order_state: None,
                direction: None,
                order_type: Some(type_str.to_string()),
                time_in_force: None,
                price: None,
                amount: None,
                filled_amount: None,
                average_price: None,
                creation_timestamp: None,
                last_update_timestamp: None,
                label: None,
                trigger_price: None,
                stop_price: None,
                reduce_only: None,
                post_only: None,
                profit_loss: None,
                commission: None,
            };

            let event = DeribitWs::parse_order_update(&data);
            assert_eq!(
                event.order.order_type, expected,
                "Type {type_str} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn test_private_data_types() {
        // Test order data deserialization
        let order_json = r#"{"order_id":"abc","instrument_name":"BTC-PERPETUAL","direction":"buy","amount":100}"#;
        let order: DeribitWsOrderData = serde_json::from_str(order_json).unwrap();
        assert_eq!(order.order_id, Some("abc".to_string()));
        assert_eq!(order.instrument_name, "BTC-PERPETUAL");

        // Test user trade data deserialization
        let trade_json = r#"{"trade_id":"xyz","instrument_name":"BTC-PERPETUAL","timestamp":1705315800000,"direction":"buy","price":50000.0,"amount":10.0}"#;
        let trade: DeribitWsUserTradeData = serde_json::from_str(trade_json).unwrap();
        assert_eq!(trade.trade_id, "xyz");
        assert_eq!(trade.direction, "buy");

        // Test position data deserialization
        let pos_json =
            r#"{"instrument_name":"BTC-PERPETUAL","size":1000.0,"average_price":50000.0}"#;
        let pos: DeribitWsPositionData = serde_json::from_str(pos_json).unwrap();
        assert_eq!(pos.instrument_name, "BTC-PERPETUAL");
        assert_eq!(pos.size, Some(1000.0));

        // Test portfolio data deserialization
        let portfolio_json = r#"{"currency":"BTC","balance":1.5,"available_funds":1.2}"#;
        let portfolio: DeribitWsPortfolioData = serde_json::from_str(portfolio_json).unwrap();
        assert_eq!(portfolio.currency, "BTC");
        assert_eq!(portfolio.balance, Some(1.5));
    }

    #[test]
    fn test_with_config_creation() {
        let config = ExchangeConfig::new()
            .with_api_key("test_key")
            .with_api_secret("test_secret")
            .with_sandbox(true);

        let ws = DeribitWs::with_config(config);
        assert!(ws.testnet);
        assert!(ws.config.is_some());
    }

    #[test]
    fn test_with_credentials() {
        let ws = DeribitWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = DeribitWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
        let config = ws.config.unwrap();
        assert_eq!(config.api_key(), Some("test_key"));
        assert_eq!(config.secret(), Some("test_secret"));
    }
}
