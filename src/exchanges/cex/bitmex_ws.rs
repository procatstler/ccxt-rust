//! BitMEX WebSocket Implementation
//!
//! BitMEX WebSocket API for real-time market data and private user data streaming

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType,
    Position, PositionSide, TakerOrMaker, Ticker, Timeframe, Trade, WsBalanceEvent, WsExchange,
    WsMessage, WsMyTradeEvent, WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsPositionEvent,
    WsTickerEvent, WsTradeEvent, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const WS_BASE_URL: &str = "wss://www.bitmex.com/realtime";
const WS_TESTNET_URL: &str = "wss://testnet.bitmex.com/realtime";

/// BitMEX WebSocket client
pub struct BitmexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    authenticated: Arc<RwLock<bool>>,
    testnet: bool,
}

impl BitmexWs {
    /// Create new BitMEX WebSocket client
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
            testnet: false,
        }
    }

    /// Create new BitMEX WebSocket client with config
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
            testnet: false,
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

    /// Create new BitMEX WebSocket client with testnet flag
    pub fn with_testnet(testnet: bool) -> Self {
        Self {
            config: None,
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            authenticated: Arc::new(RwLock::new(false)),
            testnet,
        }
    }

    /// Get WebSocket URL based on testnet flag
    fn get_url(&self) -> &'static str {
        if self.testnet {
            WS_TESTNET_URL
        } else {
            WS_BASE_URL
        }
    }

    /// Timeframe to BitMEX tradeBin interval mapping
    fn format_interval(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Hour1 => "1h",
            Timeframe::Day1 => "1d",
            _ => "1m", // Default to 1 minute
        }
    }

    /// Parse unified symbol from BitMEX symbol
    fn to_unified_symbol(symbol: &str) -> String {
        // BitMEX format: XBTUSD (perpetual), XBTM23 (futures)
        // Convert to unified: BTC/USD:BTC
        if let Some(rest) = symbol.strip_prefix("XBT") {
            if symbol == "XBTUSD" || symbol.ends_with("USD") {
                return "BTC/USD:BTC".to_string();
            }
            // Futures contract
            return format!("BTC/USD:BTC:{rest}");
        }

        if let Some(rest) = symbol.strip_prefix("ETH") {
            if symbol == "ETHUSD" {
                return "ETH/USD:ETH".to_string();
            }
            return format!("ETH/USD:ETH:{rest}");
        }

        // Default: keep as is
        symbol.to_string()
    }

    /// Parse BitMEX symbol to unified symbol
    fn from_unified_symbol(symbol: &str) -> String {
        // Convert unified symbol back to BitMEX format
        // BTC/USD:BTC -> XBTUSD
        // ETH/USD:ETH -> ETHUSD
        if symbol.starts_with("BTC/USD") {
            if symbol == "BTC/USD:BTC" {
                return "XBTUSD".to_string();
            }
            // Extract contract date/type if present
            let parts: Vec<&str> = symbol.split(':').collect();
            if parts.len() > 2 {
                return format!("XBT{}", parts[2]);
            }
            return "XBTUSD".to_string();
        }

        if symbol.starts_with("ETH/USD") {
            if symbol == "ETH/USD:ETH" {
                return "ETHUSD".to_string();
            }
            let parts: Vec<&str> = symbol.split(':').collect();
            if parts.len() > 2 {
                return format!("ETH{}", parts[2]);
            }
            return "ETHUSD".to_string();
        }

        symbol.to_string()
    }

    /// Generate authentication signature
    fn generate_auth_signature(&self, expires: i64) -> CcxtResult<String> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key and secret required for authentication".into(),
            })?;

        let secret = config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".into(),
            })?;

        let message = format!("GET/realtime{expires}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("Invalid secret key: {e}"),
            }
        })?;

        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        Ok(signature)
    }

    /// Authenticate WebSocket connection
    async fn authenticate(&self) -> CcxtResult<()> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required for authentication".into(),
            })?;

        let api_key = config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        let expires = Utc::now().timestamp() + 60; // 60 seconds from now
        let signature = self.generate_auth_signature(expires)?;

        let auth_msg = serde_json::json!({
            "op": "authKeyExpires",
            "args": [api_key, expires, signature]
        });

        if let Some(client) = &self.ws_client {
            let msg_str = serde_json::to_string(&auth_msg).map_err(|e| CcxtError::ParseError {
                data_type: "AuthMessage".to_string(),
                message: e.to_string(),
            })?;
            client.send(&msg_str)?;
        }

        *self.authenticated.write().await = true;
        Ok(())
    }

    /// Parse ticker message
    fn parse_ticker(data: &BitmexInstrumentData) -> WsTickerEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .clone()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker = Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price.and_then(Decimal::from_f64_retain),
            low: data.low_price.and_then(Decimal::from_f64_retain),
            bid: data.bid_price.and_then(Decimal::from_f64_retain),
            bid_volume: None,
            ask: data.ask_price.and_then(Decimal::from_f64_retain),
            ask_volume: None,
            vwap: data.vwap.and_then(Decimal::from_f64_retain),
            open: None,
            close: data.last_price.and_then(Decimal::from_f64_retain),
            last: data.last_price.and_then(Decimal::from_f64_retain),
            previous_close: data.prev_close_price.and_then(Decimal::from_f64_retain),
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.map(|v| Decimal::new(v, 0)),
            quote_volume: data.volume_usd.map(|v| Decimal::new(v, 0)),
            index_price: None,
            mark_price: data.mark_price.and_then(Decimal::from_f64_retain),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsTickerEvent { symbol, ticker }
    }

    /// Parse order book message
    fn parse_order_book(
        data: &BitmexOrderBookData,
        symbol: &str,
        is_snapshot: bool,
    ) -> WsOrderBookEvent {
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                Some(OrderBookEntry {
                    price: Decimal::from_f64_retain(b.price)?,
                    amount: Decimal::new(b.size, 0),
                })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                Some(OrderBookEntry {
                    price: Decimal::from_f64_retain(a.price)?,
                    amount: Decimal::new(a.size, 0),
                })
            })
            .collect();

        let timestamp = data
            .timestamp
            .clone()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
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
            checksum: None,
        };

        WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        }
    }

    /// Parse trade message
    fn parse_trades(trades: &[BitmexTradeData]) -> Option<WsTradeEvent> {
        if trades.is_empty() {
            return None;
        }

        let first_trade = &trades[0];
        let symbol = Self::to_unified_symbol(&first_trade.symbol);

        let parsed_trades: Vec<Trade> = trades
            .iter()
            .map(|t| {
                let timestamp = t
                    .timestamp
                    .clone()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let side = t.side.as_ref().map(|s| s.to_lowercase());
                let price = Decimal::from_f64_retain(t.price).unwrap_or_default();
                let amount = Decimal::new(t.size, 0);

                Trade {
                    id: t.trd_match_id.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.clone(),
                    trade_type: None,
                    side,
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Some(WsTradeEvent {
            symbol,
            trades: parsed_trades,
        })
    }

    /// Parse OHLCV message (tradeBin)
    fn parse_ohlcv(data: &BitmexTradeBinData, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        let symbol = Self::to_unified_symbol(&data.symbol);

        let timestamp = data
            .timestamp
            .clone()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let ohlcv = OHLCV {
            timestamp,
            open: Decimal::from_f64_retain(data.open)?,
            high: Decimal::from_f64_retain(data.high)?,
            low: Decimal::from_f64_retain(data.low)?,
            close: Decimal::from_f64_retain(data.close)?,
            volume: Decimal::new(data.volume, 0),
        };

        Some(WsOhlcvEvent {
            symbol,
            timeframe,
            ohlcv,
        })
    }

    /// Parse order update from private WebSocket
    fn parse_order_update(data: &BitmexWsOrderData) -> WsOrderEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);

        let timestamp = data
            .timestamp
            .clone()
            .or_else(|| data.transact_time.clone())
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = data
            .side
            .as_ref()
            .map(|s| match s.to_lowercase().as_str() {
                "buy" => OrderSide::Buy,
                "sell" => OrderSide::Sell,
                _ => OrderSide::Buy,
            })
            .unwrap_or(OrderSide::Buy);

        let order_type = data
            .ord_type
            .as_ref()
            .map(|t| match t.as_str() {
                "Market" => OrderType::Market,
                "Limit" => OrderType::Limit,
                "Stop" => OrderType::StopLoss,
                "StopLimit" => OrderType::StopLossLimit,
                "MarketIfTouched" => OrderType::TakeProfit,
                "LimitIfTouched" => OrderType::TakeProfitLimit,
                _ => OrderType::Limit,
            })
            .unwrap_or(OrderType::Limit);

        let status = data
            .ord_status
            .as_ref()
            .map(|s| match s.as_str() {
                "New" => OrderStatus::Open,
                "PartiallyFilled" => OrderStatus::Open,
                "Filled" => OrderStatus::Closed,
                "Canceled" => OrderStatus::Canceled,
                "Rejected" => OrderStatus::Rejected,
                "Expired" => OrderStatus::Expired,
                _ => OrderStatus::Open,
            })
            .unwrap_or(OrderStatus::Open);

        let price = data.price.and_then(Decimal::from_f64_retain);
        let amount = data
            .order_qty
            .map(|q| Decimal::new(q, 0))
            .unwrap_or_default();
        let filled = data.cum_qty.map(|q| Decimal::new(q, 0)).unwrap_or_default();
        let remaining = data.leaves_qty.map(|q| Decimal::new(q, 0));
        let average = data.avg_px.and_then(Decimal::from_f64_retain);
        let stop_price = data.stop_px.and_then(Decimal::from_f64_retain);

        let order = Order {
            id: data.order_id.clone(),
            client_order_id: data.cl_ord_id.clone(),
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
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price,
            trigger_price: stop_price,
            stop_loss_price: None,
            take_profit_price: None,
            cost: average.map(|a| a * filled),
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsOrderEvent { order }
    }

    /// Parse execution (my trade) from private WebSocket
    fn parse_my_trade(data: &BitmexWsExecutionData) -> WsMyTradeEvent {
        let symbol = Self::to_unified_symbol(&data.symbol);

        let timestamp = data
            .timestamp
            .clone()
            .or_else(|| data.transact_time.clone())
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let side = data.side.as_ref().map(|s| s.to_lowercase());

        let price = data
            .last_px
            .and_then(Decimal::from_f64_retain)
            .unwrap_or_default();
        let amount = data
            .last_qty
            .map(|q| Decimal::new(q, 0))
            .unwrap_or_default();

        // BitMEX commission is in satoshis (1e-8 BTC)
        let fee_cost = data.exec_comm.map(|c| {
            Decimal::new(c, 8) // Convert satoshis to BTC
        });

        let fee = fee_cost.map(|cost| Fee {
            currency: Some("XBT".to_string()),
            cost: Some(cost),
            rate: data.commission.and_then(Decimal::from_f64_retain),
        });

        // Determine taker_or_maker based on exec_type
        let taker_or_maker = data.exec_type.as_ref().and_then(|t| match t.as_str() {
            "Trade" => Some(TakerOrMaker::Taker),
            _ => None,
        });

        let cost = if price != Decimal::ZERO && amount != Decimal::ZERO {
            Some(price * amount)
        } else {
            None
        };

        let trade = Trade {
            id: data.exec_id.clone(),
            order: Some(data.order_id.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost,
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        WsMyTradeEvent {
            symbol,
            trades: vec![trade],
        }
    }

    /// Parse position update from private WebSocket
    fn parse_position_update(positions: &[BitmexWsPositionData]) -> WsPositionEvent {
        let parsed: Vec<Position> = positions
            .iter()
            .filter(|p| p.current_qty.map(|q| q != 0).unwrap_or(false))
            .map(|p| {
                let symbol = Self::to_unified_symbol(&p.symbol);

                let timestamp = p
                    .timestamp
                    .clone()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let contracts = p.current_qty.map(|q| Decimal::new(q.abs(), 0));
                let entry_price = p.avg_entry_price.and_then(Decimal::from_f64_retain);
                let mark_price = p.mark_price.and_then(Decimal::from_f64_retain);
                let liquidation_price = p.liquidation_price.and_then(Decimal::from_f64_retain);
                let leverage = p.leverage.and_then(Decimal::from_f64_retain);

                // PnL in satoshis
                let unrealized_pnl = p.unrealised_pnl.map(|pnl| Decimal::new(pnl, 8));
                let realized_pnl = p.realised_pnl.map(|pnl| Decimal::new(pnl, 8));

                let side = p.current_qty.map(|q| {
                    if q > 0 {
                        PositionSide::Long
                    } else {
                        PositionSide::Short
                    }
                });

                Position {
                    id: None,
                    symbol,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    hedged: None,
                    side,
                    contracts,
                    contract_size: None,
                    entry_price,
                    mark_price,
                    notional: None,
                    leverage,
                    collateral: None,
                    initial_margin: None,
                    initial_margin_percentage: None,
                    maintenance_margin: None,
                    maintenance_margin_percentage: None,
                    margin_ratio: None,
                    unrealized_pnl,
                    realized_pnl,
                    liquidation_price,
                    margin_mode: if p.cross_margin.unwrap_or(true) {
                        Some(crate::types::MarginMode::Cross)
                    } else {
                        Some(crate::types::MarginMode::Isolated)
                    },
                    last_update_timestamp: Some(timestamp),
                    last_price: None,
                    stop_loss_price: None,
                    take_profit_price: None,
                    percentage: None,
                    info: serde_json::to_value(p).unwrap_or_default(),
                }
            })
            .collect();

        WsPositionEvent { positions: parsed }
    }

    /// Parse wallet (balance) update from private WebSocket
    fn parse_wallet_update(wallets: &[BitmexWsWalletData]) -> WsBalanceEvent {
        let timestamp = Utc::now().timestamp_millis();
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for wallet in wallets {
            // BitMEX amounts are in satoshis for XBt
            let currency = if wallet.currency == "XBt" {
                "BTC".to_string()
            } else {
                wallet.currency.clone()
            };

            // Convert satoshis to BTC (1 XBt = 1e-8 BTC)
            let free_amount = wallet.amount.map(|a| Decimal::new(a, 8));
            let pending = wallet.pending_credit.unwrap_or(0) + wallet.pending_debit.unwrap_or(0);
            let used_amount = Some(Decimal::new(pending.abs(), 8));

            let balance = Balance {
                free: free_amount,
                used: used_amount,
                total: free_amount,
                debt: None,
            };

            currencies.insert(currency, balance);
        }

        let balances = Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::Value::Null,
        };

        WsBalanceEvent { balances }
    }

    /// Process incoming WebSocket messages
    fn process_message(msg: &str) -> Option<WsMessage> {
        let json: BitmexMessage = match serde_json::from_str(msg) {
            Ok(m) => m,
            Err(_) => return None,
        };

        // Handle subscription success
        if json.success == Some(true) {
            if let Some(subscribe) = json.subscribe {
                return Some(WsMessage::Subscribed {
                    channel: subscribe,
                    symbol: None,
                });
            }
        }

        // Handle errors
        if let Some(error) = json.error {
            return Some(WsMessage::Error(error));
        }

        // Handle authentication success
        if json.success == Some(true) && json.request == Some("authKeyExpires".to_string()) {
            return Some(WsMessage::Authenticated);
        }

        // Handle data updates
        if let Some(table) = json.table {
            if let Some(data) = json.data {
                let action = json.action.as_deref();

                match table.as_str() {
                    "instrument" => {
                        if let Ok(instruments) =
                            serde_json::from_value::<Vec<BitmexInstrumentData>>(data)
                        {
                            if let Some(instrument) = instruments.first() {
                                return Some(WsMessage::Ticker(Self::parse_ticker(instrument)));
                            }
                        }
                    },
                    "orderBook10" | "orderBookL2" | "orderBookL2_25" => {
                        // For orderBook updates, we need to handle partial, insert, update, delete actions
                        let is_snapshot = action == Some("partial");

                        if let Ok(book_data) =
                            serde_json::from_value::<BitmexOrderBookData>(data.clone())
                        {
                            if let Some(symbol) = book_data
                                .bids
                                .first()
                                .or(book_data.asks.first())
                                .map(|e| &e.symbol)
                            {
                                let unified_symbol = Self::to_unified_symbol(symbol);
                                return Some(WsMessage::OrderBook(Self::parse_order_book(
                                    &book_data,
                                    &unified_symbol,
                                    is_snapshot,
                                )));
                            }
                        }
                    },
                    "trade" => {
                        if let Ok(trades) = serde_json::from_value::<Vec<BitmexTradeData>>(data) {
                            if let Some(event) = Self::parse_trades(&trades) {
                                return Some(WsMessage::Trade(event));
                            }
                        }
                    },
                    "tradeBin1m" | "tradeBin5m" | "tradeBin1h" | "tradeBin1d" => {
                        let timeframe = match table.as_str() {
                            "tradeBin1m" => Timeframe::Minute1,
                            "tradeBin5m" => Timeframe::Minute5,
                            "tradeBin1h" => Timeframe::Hour1,
                            "tradeBin1d" => Timeframe::Day1,
                            _ => Timeframe::Minute1,
                        };

                        if let Ok(bins) = serde_json::from_value::<Vec<BitmexTradeBinData>>(data) {
                            if let Some(bin) = bins.first() {
                                if let Some(event) = Self::parse_ohlcv(bin, timeframe) {
                                    return Some(WsMessage::Ohlcv(event));
                                }
                            }
                        }
                    },
                    // === Private channels ===
                    "order" => {
                        if let Ok(orders) = serde_json::from_value::<Vec<BitmexWsOrderData>>(data) {
                            if let Some(order) = orders.first() {
                                return Some(WsMessage::Order(Self::parse_order_update(order)));
                            }
                        }
                    },
                    "execution" => {
                        // Filter to only Trade executions (not funding, settlement, etc.)
                        if let Ok(executions) =
                            serde_json::from_value::<Vec<BitmexWsExecutionData>>(data)
                        {
                            for exec in executions {
                                if exec.exec_type.as_deref() == Some("Trade") {
                                    return Some(WsMessage::MyTrade(Self::parse_my_trade(&exec)));
                                }
                            }
                        }
                    },
                    "position" => {
                        if let Ok(positions) =
                            serde_json::from_value::<Vec<BitmexWsPositionData>>(data)
                        {
                            if !positions.is_empty() {
                                return Some(WsMessage::Position(Self::parse_position_update(
                                    &positions,
                                )));
                            }
                        }
                    },
                    "wallet" => {
                        if let Ok(wallets) = serde_json::from_value::<Vec<BitmexWsWalletData>>(data)
                        {
                            if !wallets.is_empty() {
                                return Some(WsMessage::Balance(Self::parse_wallet_update(
                                    &wallets,
                                )));
                            }
                        }
                    },
                    _ => {},
                }
            }
        }

        None
    }

    /// Subscribe to channel and return event receiver
    async fn subscribe_channel(
        &mut self,
        topics: Vec<String>,
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

        // Authenticate if config is available
        if self.config.is_some() {
            self.authenticate().await?;
        }

        // Send subscription request
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": topics
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
        for topic in topics {
            self.subscriptions
                .write()
                .await
                .insert(topic.clone(), topic);
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

    /// Subscribe to private channel (requires authentication)
    async fn subscribe_private_channel(
        &mut self,
        topics: Vec<String>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        // Ensure we have credentials
        if self.config.is_none() {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        }

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

        // Send subscription request
        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": topics
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
        for topic in topics {
            self.subscriptions
                .write()
                .await
                .insert(topic.clone(), topic);
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

impl Default for BitmexWs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsExchange for BitmexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);
        let topic = format!("instrument:{bitmex_symbol}");
        client.subscribe_channel(vec![topic]).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let topics: Vec<String> = symbols
            .iter()
            .map(|s| {
                let bitmex_symbol = Self::from_unified_symbol(s);
                format!("instrument:{bitmex_symbol}")
            })
            .collect();
        client.subscribe_channel(topics).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);

        // Choose orderBook channel based on limit
        let channel = match limit {
            Some(10) | None => "orderBook10", // Top 10 levels (default)
            Some(25) => "orderBookL2_25",     // Top 25 levels
            _ => "orderBookL2",               // Full order book
        };

        let topic = format!("{channel}:{bitmex_symbol}");
        client.subscribe_channel(vec![topic]).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);
        let topic = format!("trade:{bitmex_symbol}");
        client.subscribe_channel(vec![topic]).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let bitmex_symbol = Self::from_unified_symbol(symbol);
        let interval = Self::format_interval(timeframe);
        let topic = format!("tradeBin{interval}:{bitmex_symbol}");
        client.subscribe_channel(vec![topic]).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // BitMEX connects on subscription
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(client) = &self.ws_client {
            client.close()?;
        }
        self.ws_client = None;
        *self.authenticated.write().await = false;
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
        client.testnet = self.testnet;

        let topic = if let Some(sym) = symbol {
            let bitmex_symbol = Self::from_unified_symbol(sym);
            format!("order:{bitmex_symbol}")
        } else {
            "order".to_string()
        };

        client.subscribe_private_channel(vec![topic]).await
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
        client.testnet = self.testnet;

        let topic = if let Some(sym) = symbol {
            let bitmex_symbol = Self::from_unified_symbol(sym);
            format!("execution:{bitmex_symbol}")
        } else {
            "execution".to_string()
        };

        client.subscribe_private_channel(vec![topic]).await
    }

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = if let Some(config) = &self.config {
            Self::with_config(config.clone())
        } else {
            return Err(CcxtError::AuthenticationError {
                message: "API key and secret required for private channels".into(),
            });
        };
        client.testnet = self.testnet;

        client
            .subscribe_private_channel(vec!["wallet".to_string()])
            .await
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
        client.testnet = self.testnet;

        let topics: Vec<String> = if let Some(syms) = symbols {
            syms.iter()
                .map(|s| {
                    let bitmex_symbol = Self::from_unified_symbol(s);
                    format!("position:{bitmex_symbol}")
                })
                .collect()
        } else {
            vec!["position".to_string()]
        };

        client.subscribe_private_channel(topics).await
    }
}

// === BitMEX WebSocket Message Types ===

#[derive(Debug, Deserialize)]
struct BitmexMessage {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    subscribe: Option<String>,
    #[serde(default)]
    request: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexInstrumentData {
    symbol: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    last_price: Option<f64>,
    #[serde(default)]
    bid_price: Option<f64>,
    #[serde(default)]
    ask_price: Option<f64>,
    #[serde(default)]
    mark_price: Option<f64>,
    #[serde(default)]
    high_price: Option<f64>,
    #[serde(default)]
    low_price: Option<f64>,
    #[serde(default)]
    prev_close_price: Option<f64>,
    #[serde(default)]
    volume: Option<i64>,
    #[serde(default)]
    volume_usd: Option<i64>,
    #[serde(default)]
    vwap: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BitmexOrderBookData {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    bids: Vec<BitmexOrderBookLevel>,
    #[serde(default)]
    asks: Vec<BitmexOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct BitmexOrderBookLevel {
    symbol: String,
    price: f64,
    size: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexTradeData {
    #[serde(default)]
    timestamp: Option<String>,
    symbol: String,
    #[serde(default)]
    side: Option<String>,
    size: i64,
    price: f64,
    #[serde(default)]
    trd_match_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitmexTradeBinData {
    timestamp: Option<String>,
    symbol: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: i64,
}

// === Private WebSocket Data Types ===

#[derive(Debug, Deserialize, Serialize, Clone)]
struct BitmexWsOrderData {
    #[serde(rename = "orderID")]
    order_id: String,
    #[serde(rename = "clOrdID")]
    cl_ord_id: Option<String>,
    symbol: String,
    side: Option<String>,
    #[serde(rename = "orderQty")]
    order_qty: Option<i64>,
    price: Option<f64>,
    #[serde(rename = "stopPx")]
    stop_px: Option<f64>,
    #[serde(rename = "ordType")]
    ord_type: Option<String>,
    #[serde(rename = "ordStatus")]
    ord_status: Option<String>,
    #[serde(rename = "leavesQty")]
    leaves_qty: Option<i64>,
    #[serde(rename = "cumQty")]
    cum_qty: Option<i64>,
    #[serde(rename = "avgPx")]
    avg_px: Option<f64>,
    #[serde(rename = "transactTime")]
    transact_time: Option<String>,
    timestamp: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct BitmexWsExecutionData {
    #[serde(rename = "execID")]
    exec_id: String,
    #[serde(rename = "orderID")]
    order_id: String,
    #[serde(rename = "clOrdID")]
    cl_ord_id: Option<String>,
    symbol: String,
    side: Option<String>,
    #[serde(rename = "lastQty")]
    last_qty: Option<i64>,
    #[serde(rename = "lastPx")]
    last_px: Option<f64>,
    #[serde(rename = "execType")]
    exec_type: Option<String>,
    #[serde(rename = "ordStatus")]
    ord_status: Option<String>,
    #[serde(rename = "execComm")]
    exec_comm: Option<i64>,
    #[serde(rename = "commission")]
    commission: Option<f64>,
    #[serde(rename = "transactTime")]
    transact_time: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "trdMatchID")]
    trd_match_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct BitmexWsPositionData {
    account: Option<i64>,
    symbol: String,
    currency: Option<String>,
    #[serde(rename = "currentQty")]
    current_qty: Option<i64>,
    #[serde(rename = "avgEntryPrice")]
    avg_entry_price: Option<f64>,
    #[serde(rename = "markPrice")]
    mark_price: Option<f64>,
    #[serde(rename = "liquidationPrice")]
    liquidation_price: Option<f64>,
    leverage: Option<f64>,
    #[serde(rename = "unrealisedPnl")]
    unrealised_pnl: Option<i64>,
    #[serde(rename = "realisedPnl")]
    realised_pnl: Option<i64>,
    #[serde(rename = "crossMargin")]
    cross_margin: Option<bool>,
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct BitmexWsWalletData {
    account: Option<i64>,
    currency: String,
    amount: Option<i64>,
    #[serde(rename = "pendingCredit")]
    pending_credit: Option<i64>,
    #[serde(rename = "pendingDebit")]
    pending_debit: Option<i64>,
    timestamp: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitmexWs::to_unified_symbol("XBTUSD"), "BTC/USD:BTC");
        assert_eq!(BitmexWs::to_unified_symbol("ETHUSD"), "ETH/USD:ETH");
        assert_eq!(BitmexWs::to_unified_symbol("XBTM23"), "BTC/USD:BTC:M23");
        assert_eq!(BitmexWs::to_unified_symbol("SOLUSDT"), "SOLUSDT");
    }

    #[test]
    fn test_from_unified_symbol() {
        assert_eq!(BitmexWs::from_unified_symbol("BTC/USD:BTC"), "XBTUSD");
        assert_eq!(BitmexWs::from_unified_symbol("ETH/USD:ETH"), "ETHUSD");
        assert_eq!(BitmexWs::from_unified_symbol("BTC/USD:BTC:M23"), "XBTM23");
    }

    #[test]
    fn test_parse_order_update() {
        let data = BitmexWsOrderData {
            order_id: "order-123".to_string(),
            cl_ord_id: Some("client-123".to_string()),
            symbol: "XBTUSD".to_string(),
            side: Some("Buy".to_string()),
            order_qty: Some(100),
            price: Some(50000.0),
            stop_px: None,
            ord_type: Some("Limit".to_string()),
            ord_status: Some("New".to_string()),
            leaves_qty: Some(100),
            cum_qty: Some(0),
            avg_px: None,
            transact_time: Some("2024-01-15T10:30:00.000Z".to_string()),
            timestamp: Some("2024-01-15T10:30:00.000Z".to_string()),
            text: None,
        };

        let event = BitmexWs::parse_order_update(&data);
        assert_eq!(event.order.id, "order-123");
        assert_eq!(event.order.symbol, "BTC/USD:BTC");
        assert_eq!(event.order.side, OrderSide::Buy);
        assert_eq!(event.order.order_type, OrderType::Limit);
        assert_eq!(event.order.status, OrderStatus::Open);
        assert_eq!(event.order.amount, Decimal::new(100, 0));
    }

    #[test]
    fn test_parse_order_update_filled() {
        let data = BitmexWsOrderData {
            order_id: "order-456".to_string(),
            cl_ord_id: None,
            symbol: "ETHUSD".to_string(),
            side: Some("Sell".to_string()),
            order_qty: Some(50),
            price: Some(3000.0),
            stop_px: None,
            ord_type: Some("Market".to_string()),
            ord_status: Some("Filled".to_string()),
            leaves_qty: Some(0),
            cum_qty: Some(50),
            avg_px: Some(3001.5),
            transact_time: Some("2024-01-15T10:35:00.000Z".to_string()),
            timestamp: None,
            text: None,
        };

        let event = BitmexWs::parse_order_update(&data);
        assert_eq!(event.order.status, OrderStatus::Closed);
        assert_eq!(event.order.side, OrderSide::Sell);
        assert_eq!(event.order.order_type, OrderType::Market);
        assert_eq!(event.order.filled, Decimal::new(50, 0));
    }

    #[test]
    fn test_parse_my_trade() {
        let data = BitmexWsExecutionData {
            exec_id: "exec-789".to_string(),
            order_id: "order-123".to_string(),
            cl_ord_id: Some("client-123".to_string()),
            symbol: "XBTUSD".to_string(),
            side: Some("Buy".to_string()),
            last_qty: Some(10),
            last_px: Some(50000.0),
            exec_type: Some("Trade".to_string()),
            ord_status: Some("Filled".to_string()),
            exec_comm: Some(750000), // 0.0075 BTC in satoshis
            commission: Some(0.00075),
            transact_time: Some("2024-01-15T10:30:00.000Z".to_string()),
            timestamp: Some("2024-01-15T10:30:00.000Z".to_string()),
            trd_match_id: Some("match-001".to_string()),
        };

        let event = BitmexWs::parse_my_trade(&data);
        assert_eq!(event.symbol, "BTC/USD:BTC");
        assert_eq!(event.trades.len(), 1);

        let trade = &event.trades[0];
        assert_eq!(trade.id, "exec-789");
        assert_eq!(trade.order, Some("order-123".to_string()));
        assert_eq!(trade.side, Some("buy".to_string()));
        assert_eq!(trade.price, Decimal::new(50000, 0));
        assert_eq!(trade.amount, Decimal::new(10, 0));
        assert!(trade.fee.is_some());
    }

    #[test]
    fn test_parse_position_update() {
        let positions = vec![BitmexWsPositionData {
            account: Some(12345),
            symbol: "XBTUSD".to_string(),
            currency: Some("XBt".to_string()),
            current_qty: Some(1000),
            avg_entry_price: Some(50000.0),
            mark_price: Some(50500.0),
            liquidation_price: Some(40000.0),
            leverage: Some(10.0),
            unrealised_pnl: Some(10000000), // 0.1 BTC in satoshis
            realised_pnl: Some(5000000),
            cross_margin: Some(false),
            timestamp: Some("2024-01-15T10:30:00.000Z".to_string()),
        }];

        let event = BitmexWs::parse_position_update(&positions);
        assert_eq!(event.positions.len(), 1);

        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "BTC/USD:BTC");
        assert_eq!(pos.side, Some(PositionSide::Long));
        assert_eq!(pos.contracts, Some(Decimal::new(1000, 0)));
        assert_eq!(pos.leverage, Some(Decimal::new(10, 0)));
        // cross_margin = false means isolated margin mode
    }

    #[test]
    fn test_parse_position_update_short() {
        let positions = vec![BitmexWsPositionData {
            account: Some(12345),
            symbol: "ETHUSD".to_string(),
            currency: Some("XBt".to_string()),
            current_qty: Some(-500),
            avg_entry_price: Some(3000.0),
            mark_price: Some(2950.0),
            liquidation_price: Some(3500.0),
            leverage: Some(5.0),
            unrealised_pnl: Some(2500000),
            realised_pnl: None,
            cross_margin: Some(true),
            timestamp: None,
        }];

        let event = BitmexWs::parse_position_update(&positions);
        assert_eq!(event.positions.len(), 1);

        let pos = &event.positions[0];
        assert_eq!(pos.symbol, "ETH/USD:ETH");
        assert_eq!(pos.side, Some(PositionSide::Short));
        assert_eq!(pos.contracts, Some(Decimal::new(500, 0)));
        // cross_margin = true means cross margin mode
    }

    #[test]
    fn test_parse_wallet_update() {
        let wallets = vec![BitmexWsWalletData {
            account: Some(12345),
            currency: "XBt".to_string(),
            amount: Some(100000000), // 1 BTC in satoshis
            pending_credit: Some(0),
            pending_debit: Some(0),
            timestamp: Some("2024-01-15T10:30:00.000Z".to_string()),
        }];

        let event = BitmexWs::parse_wallet_update(&wallets);
        let btc_balance = event.balances.currencies.get("BTC");
        assert!(btc_balance.is_some());

        let balance = btc_balance.unwrap();
        assert_eq!(balance.free, Some(Decimal::new(1, 0))); // 1 BTC
    }

    #[test]
    fn test_process_message_order() {
        let msg = r#"{"table":"order","action":"insert","data":[{"orderID":"order-123","symbol":"XBTUSD","side":"Buy","orderQty":100,"price":50000.0,"ordType":"Limit","ordStatus":"New","leavesQty":100,"cumQty":0,"timestamp":"2024-01-15T10:30:00.000Z"}]}"#;

        let result = BitmexWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "order-123");
            assert_eq!(event.order.symbol, "BTC/USD:BTC");
        } else {
            panic!("Expected Order message");
        }
    }

    #[test]
    fn test_process_message_execution() {
        let msg = r#"{"table":"execution","action":"insert","data":[{"execID":"exec-456","orderID":"order-123","symbol":"XBTUSD","side":"Buy","lastQty":10,"lastPx":50000.0,"execType":"Trade","ordStatus":"Filled","timestamp":"2024-01-15T10:30:00.000Z"}]}"#;

        let result = BitmexWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::MyTrade(event)) = result {
            assert_eq!(event.symbol, "BTC/USD:BTC");
            assert!(!event.trades.is_empty());
        } else {
            panic!("Expected MyTrade message");
        }
    }

    #[test]
    fn test_process_message_position() {
        let msg = r#"{"table":"position","action":"update","data":[{"symbol":"XBTUSD","currentQty":1000,"avgEntryPrice":50000.0,"markPrice":50500.0,"leverage":10.0,"timestamp":"2024-01-15T10:30:00.000Z"}]}"#;

        let result = BitmexWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Position(event)) = result {
            assert!(!event.positions.is_empty());
            assert_eq!(event.positions[0].symbol, "BTC/USD:BTC");
        } else {
            panic!("Expected Position message");
        }
    }

    #[test]
    fn test_process_message_wallet() {
        let msg = r#"{"table":"wallet","action":"partial","data":[{"currency":"XBt","amount":100000000,"timestamp":"2024-01-15T10:30:00.000Z"}]}"#;

        let result = BitmexWs::process_message(msg);
        assert!(result.is_some());

        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
        } else {
            panic!("Expected Balance message");
        }
    }

    #[test]
    fn test_process_message_auth_success() {
        let msg = r#"{"success":true,"request":"authKeyExpires"}"#;

        let result = BitmexWs::process_message(msg);
        assert!(matches!(result, Some(WsMessage::Authenticated)));
    }

    #[test]
    fn test_order_status_parsing() {
        let test_cases = vec![
            ("New", OrderStatus::Open),
            ("PartiallyFilled", OrderStatus::Open),
            ("Filled", OrderStatus::Closed),
            ("Canceled", OrderStatus::Canceled),
            ("Rejected", OrderStatus::Rejected),
            ("Expired", OrderStatus::Expired),
        ];

        for (status_str, expected) in test_cases {
            let data = BitmexWsOrderData {
                order_id: "test".to_string(),
                cl_ord_id: None,
                symbol: "XBTUSD".to_string(),
                side: None,
                order_qty: None,
                price: None,
                stop_px: None,
                ord_type: None,
                ord_status: Some(status_str.to_string()),
                leaves_qty: None,
                cum_qty: None,
                avg_px: None,
                transact_time: None,
                timestamp: None,
                text: None,
            };

            let event = BitmexWs::parse_order_update(&data);
            assert_eq!(
                event.order.status, expected,
                "Status {status_str} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn test_order_type_parsing() {
        let test_cases = vec![
            ("Market", OrderType::Market),
            ("Limit", OrderType::Limit),
            ("Stop", OrderType::StopLoss),
            ("StopLimit", OrderType::StopLossLimit),
            ("MarketIfTouched", OrderType::TakeProfit),
            ("LimitIfTouched", OrderType::TakeProfitLimit),
        ];

        for (type_str, expected) in test_cases {
            let data = BitmexWsOrderData {
                order_id: "test".to_string(),
                cl_ord_id: None,
                symbol: "XBTUSD".to_string(),
                side: None,
                order_qty: None,
                price: None,
                stop_px: None,
                ord_type: Some(type_str.to_string()),
                ord_status: None,
                leaves_qty: None,
                cum_qty: None,
                avg_px: None,
                transact_time: None,
                timestamp: None,
                text: None,
            };

            let event = BitmexWs::parse_order_update(&data);
            assert_eq!(
                event.order.order_type, expected,
                "Type {type_str} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn test_private_data_types() {
        // Test order data deserialization
        let order_json = r#"{"orderID":"abc","symbol":"XBTUSD","side":"Buy","orderQty":100}"#;
        let order: BitmexWsOrderData = serde_json::from_str(order_json).unwrap();
        assert_eq!(order.order_id, "abc");
        assert_eq!(order.symbol, "XBTUSD");

        // Test execution data deserialization
        let exec_json = r#"{"execID":"xyz","orderID":"abc","symbol":"XBTUSD","lastQty":10,"lastPx":50000.0,"execType":"Trade"}"#;
        let exec: BitmexWsExecutionData = serde_json::from_str(exec_json).unwrap();
        assert_eq!(exec.exec_id, "xyz");
        assert_eq!(exec.exec_type, Some("Trade".to_string()));

        // Test position data deserialization
        let pos_json = r#"{"symbol":"XBTUSD","currentQty":1000,"avgEntryPrice":50000.0}"#;
        let pos: BitmexWsPositionData = serde_json::from_str(pos_json).unwrap();
        assert_eq!(pos.symbol, "XBTUSD");
        assert_eq!(pos.current_qty, Some(1000));

        // Test wallet data deserialization
        let wallet_json = r#"{"currency":"XBt","amount":100000000}"#;
        let wallet: BitmexWsWalletData = serde_json::from_str(wallet_json).unwrap();
        assert_eq!(wallet.currency, "XBt");
        assert_eq!(wallet.amount, Some(100000000));
    }
}
