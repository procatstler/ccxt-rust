//! Derive WebSocket Implementation
//!
//! Derive DEX exchange WebSocket implementation using Lyra Finance API
//! API Documentation: <https://docs.derive.xyz/docs/>

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
    Balance, Balances, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus, OrderType, Ticker,
    Timeframe, Trade, WsBalanceEvent, WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent,
    WsOrderEvent, WsTickerEvent, WsTradeEvent, OHLCV,
};

const WS_URL: &str = "wss://api.lyra.finance/ws/v2";

// WebSocket message structures
#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsMessage {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<DeriveWsError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsError {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsTicker {
    #[serde(default)]
    instrument_name: Option<String>,
    #[serde(default)]
    best_bid_price: Option<String>,
    #[serde(default)]
    best_bid_amount: Option<String>,
    #[serde(default)]
    best_ask_price: Option<String>,
    #[serde(default)]
    best_ask_amount: Option<String>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
    #[serde(default)]
    open_interest: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    stats: Option<DeriveWsTickerStats>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsTickerStats {
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    volume_usd: Option<String>,
    #[serde(default)]
    price_change: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsOrderBook {
    #[serde(default)]
    instrument_name: Option<String>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsTrade {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    instrument_name: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsKline {
    #[serde(default)]
    instrument_name: Option<String>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    instrument_name: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    filled_amount: Option<String>,
    #[serde(default)]
    average_price: Option<String>,
    #[serde(default)]
    order_status: Option<String>,
    #[serde(default)]
    creation_timestamp: Option<i64>,
    #[serde(default)]
    last_update_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DeriveWsBalance {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    locked: Option<String>,
}

/// Derive WebSocket client
pub struct DeriveWs {
    ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    wallet_address: Option<String>,
    private_key: Option<String>,
    request_id: Arc<RwLock<i64>>,
}

impl DeriveWs {
    /// Create a new Derive WebSocket client
    pub fn new() -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            wallet_address: None,
            private_key: None,
            request_id: Arc::new(RwLock::new(1)),
        }
    }

    /// Set wallet credentials for private channels
    pub fn with_credentials(mut self, wallet_address: &str, private_key: &str) -> Self {
        self.wallet_address = Some(wallet_address.to_string());
        self.private_key = Some(private_key.to_string());
        self
    }

    /// Get next request ID
    async fn next_request_id(&self) -> i64 {
        let mut id = self.request_id.write().await;
        let current = *id;
        *id += 1;
        current
    }

    /// Convert unified symbol to exchange format (BTC/USD:BTC -> BTC-PERP)
    fn format_symbol(symbol: &str) -> String {
        if symbol.contains(':') {
            // Perpetual format: BTC/USD:BTC -> BTC-PERP
            let parts: Vec<&str> = symbol.split('/').collect();
            if let Some(base) = parts.first() {
                return format!("{base}-PERP");
            }
        }
        symbol.replace("/", "-")
    }

    /// Convert exchange symbol to unified format (BTC-PERP -> BTC/USD:BTC)
    fn to_unified_symbol(instrument_name: &str) -> String {
        let parts: Vec<&str> = instrument_name.split('-').collect();
        if parts.len() >= 2 {
            let base = parts[0];
            match parts.get(1) {
                Some(&"PERP") => format!("{base}/USD:{base}"),
                Some(quote) => format!("{base}/{quote}"),
                None => instrument_name.to_string(),
            }
        } else {
            instrument_name.to_string()
        }
    }

    /// Format timeframe to exchange interval
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
            Timeframe::Hour12 => "720",
            Timeframe::Day1 => "1440",
            Timeframe::Week1 => "10080",
            _ => "60",
        }
    }

    /// Parse ticker data
    fn parse_ticker(data: &DeriveWsTicker, symbol: &str) -> Ticker {
        let last = data
            .last_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let bid = data
            .best_bid_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let bid_volume = data
            .best_bid_amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let ask = data
            .best_ask_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let ask_volume = data
            .best_ask_amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let mark = data
            .mark_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let index = data
            .index_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let (high, low, base_volume, quote_volume, change) = if let Some(stats) = &data.stats {
            (
                stats.high.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                stats.low.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                stats
                    .volume
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok()),
                stats
                    .volume_usd
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok()),
                stats
                    .price_change
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok()),
            )
        } else {
            (None, None, None, None, None)
        };

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339()),
            high,
            low,
            bid,
            bid_volume,
            ask,
            ask_volume,
            vwap: None,
            open: None,
            close: last,
            last,
            previous_close: None,
            change,
            percentage: None,
            average: None,
            base_volume,
            quote_volume,
            index_price: index,
            mark_price: mark,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book data
    fn parse_order_book(data: &DeriveWsOrderBook, symbol: &str) -> OrderBook {
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = Decimal::from_str(&entry[0]).ok()?;
                    let amount = Decimal::from_str(&entry[1]).ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    let price = Decimal::from_str(&entry[0]).ok()?;
                    let amount = Decimal::from_str(&entry[1]).ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339()),
            nonce: None,
            bids,
            asks,
            checksum: None,
        }
    }

    /// Parse trade data
    fn parse_trade(data: &DeriveWsTrade, symbol: &str) -> Trade {
        let side = match data.direction.as_deref() {
            Some("buy") | Some("BUY") => Some("buy".to_string()),
            Some("sell") | Some("SELL") => Some("sell".to_string()),
            _ => None,
        };

        let price = data
            .price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let amount = data
            .amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let cost = Some(price * amount);

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp).map(|dt| dt.to_rfc3339()),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV data
    fn parse_ohlcv(data: &DeriveWsKline) -> Option<OHLCV> {
        Some(OHLCV {
            timestamp: data.timestamp?,
            open: Decimal::from_str(data.open.as_ref()?).ok()?,
            high: Decimal::from_str(data.high.as_ref()?).ok()?,
            low: Decimal::from_str(data.low.as_ref()?).ok()?,
            close: Decimal::from_str(data.close.as_ref()?).ok()?,
            volume: Decimal::from_str(data.volume.as_ref()?).ok()?,
        })
    }

    /// Parse order data
    fn parse_order(data: &DeriveWsOrder, symbol: &str) -> Order {
        let side = match data.direction.as_deref() {
            Some("buy") | Some("BUY") => OrderSide::Buy,
            Some("sell") | Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") | Some("LIMIT") => OrderType::Limit,
            Some("market") | Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match data.order_status.as_deref() {
            Some("open") | Some("OPEN") => OrderStatus::Open,
            Some("filled") | Some("FILLED") => OrderStatus::Closed,
            Some("cancelled") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("expired") | Some("EXPIRED") => OrderStatus::Expired,
            Some("rejected") | Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let amount = data
            .amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let price = data
            .limit_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let filled = data
            .filled_amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let average = data
            .average_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: data.creation_timestamp,
            datetime: data
                .creation_timestamp
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            last_trade_timestamp: data.last_update_timestamp,
            last_update_timestamp: data.last_update_timestamp,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance data
    fn parse_balance(data: &DeriveWsBalance) -> Option<(String, Balance)> {
        let currency = data.currency.clone()?;
        let free = data
            .available
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let used = data.locked.as_ref().and_then(|s| Decimal::from_str(s).ok());
        let total = data.amount.as_ref().and_then(|s| Decimal::from_str(s).ok());

        Some((
            currency,
            Balance {
                free,
                used,
                total,
                debt: None,
            },
        ))
    }

    /// Process WebSocket message
    fn process_message(msg: &str, event_tx: &mpsc::UnboundedSender<WsMessage>) -> CcxtResult<()> {
        let parsed: DeriveWsMessage = serde_json::from_str(msg)?;

        // Handle subscription data
        if let Some(method) = &parsed.method {
            if let Some(params) = &parsed.params {
                if method.as_str() == "subscription" {
                    // Check channel type from params
                    if let Some(channel) = params.get("channel").and_then(|v| v.as_str()) {
                        if let Some(data) = params.get("data") {
                            if channel.starts_with("ticker.") {
                                if let Ok(ticker_data) =
                                    serde_json::from_value::<DeriveWsTicker>(data.clone())
                                {
                                    let symbol = ticker_data
                                        .instrument_name
                                        .as_ref()
                                        .map(|s| Self::to_unified_symbol(s))
                                        .unwrap_or_default();
                                    let ticker = Self::parse_ticker(&ticker_data, &symbol);
                                    let _ = event_tx
                                        .send(WsMessage::Ticker(WsTickerEvent { symbol, ticker }));
                                }
                            } else if channel.starts_with("orderbook.") {
                                if let Ok(ob_data) =
                                    serde_json::from_value::<DeriveWsOrderBook>(data.clone())
                                {
                                    let symbol = ob_data
                                        .instrument_name
                                        .as_ref()
                                        .map(|s| Self::to_unified_symbol(s))
                                        .unwrap_or_default();
                                    let order_book = Self::parse_order_book(&ob_data, &symbol);
                                    let _ = event_tx.send(WsMessage::OrderBook(WsOrderBookEvent {
                                        symbol,
                                        order_book,
                                        is_snapshot: true,
                                    }));
                                }
                            } else if channel.starts_with("trades.") {
                                // May receive single trade or array
                                if let Ok(trade_data) =
                                    serde_json::from_value::<DeriveWsTrade>(data.clone())
                                {
                                    let symbol = trade_data
                                        .instrument_name
                                        .as_ref()
                                        .map(|s| Self::to_unified_symbol(s))
                                        .unwrap_or_default();
                                    let trade = Self::parse_trade(&trade_data, &symbol);
                                    let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                        symbol,
                                        trades: vec![trade],
                                    }));
                                } else if let Ok(trades) =
                                    serde_json::from_value::<Vec<DeriveWsTrade>>(data.clone())
                                {
                                    if let Some(first) = trades.first() {
                                        let symbol = first
                                            .instrument_name
                                            .as_ref()
                                            .map(|s| Self::to_unified_symbol(s))
                                            .unwrap_or_default();
                                        let parsed_trades: Vec<Trade> = trades
                                            .iter()
                                            .map(|t| Self::parse_trade(t, &symbol))
                                            .collect();
                                        let _ = event_tx.send(WsMessage::Trade(WsTradeEvent {
                                            symbol,
                                            trades: parsed_trades,
                                        }));
                                    }
                                }
                            } else if channel.starts_with("chart.") {
                                if let Ok(kline_data) =
                                    serde_json::from_value::<DeriveWsKline>(data.clone())
                                {
                                    let symbol = kline_data
                                        .instrument_name
                                        .as_ref()
                                        .map(|s| Self::to_unified_symbol(s))
                                        .unwrap_or_default();
                                    if let Some(ohlcv) = Self::parse_ohlcv(&kline_data) {
                                        let _ = event_tx.send(WsMessage::Ohlcv(WsOhlcvEvent {
                                            symbol,
                                            timeframe: Timeframe::Hour1,
                                            ohlcv,
                                        }));
                                    }
                                }
                            } else if channel.starts_with("user.orders") {
                                if let Ok(order_data) =
                                    serde_json::from_value::<DeriveWsOrder>(data.clone())
                                {
                                    let symbol = order_data
                                        .instrument_name
                                        .as_ref()
                                        .map(|s| Self::to_unified_symbol(s))
                                        .unwrap_or_default();
                                    let order = Self::parse_order(&order_data, &symbol);
                                    let _ = event_tx.send(WsMessage::Order(WsOrderEvent { order }));
                                }
                            } else if channel.starts_with("user.portfolio") {
                                if let Ok(balances_data) =
                                    serde_json::from_value::<Vec<DeriveWsBalance>>(data.clone())
                                {
                                    let mut balances = Balances::new();
                                    for b in &balances_data {
                                        if let Some((currency, balance)) = Self::parse_balance(b) {
                                            balances.add(&currency, balance);
                                        }
                                    }
                                    let _ = event_tx
                                        .send(WsMessage::Balance(WsBalanceEvent { balances }));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Subscribe to a channel and return message receiver
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        requires_auth: bool,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        let mut ws_client = WsClient::new(WsConfig {
            url: WS_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
            ..Default::default()
        });

        let mut ws_rx = ws_client.connect().await?;

        // Authenticate if required
        if requires_auth {
            if let (Some(wallet), Some(key)) = (&self.wallet_address, &self.private_key) {
                let timestamp = Utc::now().timestamp_millis();

                // Sign message for authentication
                let auth_msg = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": self.next_request_id().await,
                    "method": "public/auth",
                    "params": {
                        "wallet": wallet,
                        "timestamp": timestamp,
                        "signature": format!("0x{}", key)
                    }
                });

                ws_client.send(&auth_msg.to_string())?;
            }
        }

        // Subscribe to channel
        let subscribe_msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id().await,
            "method": "public/subscribe",
            "params": {
                "channels": [channel]
            }
        });

        ws_client.send(&subscribe_msg.to_string())?;

        // Record subscription
        {
            let mut subs = self.subscriptions.write().await;
            subs.insert(channel.to_string(), channel.to_string());
        }

        // Spawn message handler
        let subscriptions = self.subscriptions.clone();
        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Message(msg) => {
                        let _ = Self::process_message(&msg, &event_tx);
                    },
                    WsEvent::Connected => {},
                    WsEvent::Disconnected => {
                        break;
                    },
                    WsEvent::Error(e) => {
                        eprintln!("Derive WebSocket error: {e}");
                    },
                    WsEvent::Ping | WsEvent::Pong => {},
                    _ => {},
                }
            }
            let mut subs = subscriptions.write().await;
            subs.clear();
        });

        self.ws_client = Some(ws_client);
        Ok(event_rx)
    }

    /// Subscribe to orders (private)
    pub async fn watch_orders(
        &mut self,
        symbol: Option<&str>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let channel = if let Some(sym) = symbol {
            let instrument = Self::format_symbol(sym);
            format!("user.orders.{instrument}")
        } else {
            "user.orders".to_string()
        };
        self.subscribe_stream(&channel, true).await
    }

    /// Subscribe to balance (private)
    pub async fn watch_balance(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        self.subscribe_stream("user.portfolio", true).await
    }
}

impl Default for DeriveWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for DeriveWs {
    fn clone(&self) -> Self {
        Self {
            ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            wallet_address: self.wallet_address.clone(),
            private_key: self.private_key.clone(),
            request_id: Arc::new(RwLock::new(1)),
        }
    }
}

#[async_trait]
impl WsExchange for DeriveWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let instrument = Self::format_symbol(symbol);
        let channel = format!("ticker.{instrument}.raw");
        ws.subscribe_stream(&channel, false).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        _limit: Option<u32>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let instrument = Self::format_symbol(symbol);
        let channel = format!("orderbook.{instrument}.raw");
        ws.subscribe_stream(&channel, false).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let instrument = Self::format_symbol(symbol);
        let channel = format!("trades.{instrument}.raw");
        ws.subscribe_stream(&channel, false).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut ws = self.clone();
        let instrument = Self::format_symbol(symbol);
        let resolution = Self::format_interval(timeframe);
        let channel = format!("chart.trades.{instrument}.{resolution}");
        ws.subscribe_stream(&channel, false).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        if self.ws_client.is_none() {
            let mut ws_client = WsClient::new(WsConfig {
                url: WS_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
                ..Default::default()
            });

            let _ = ws_client.connect().await?;
            self.ws_client = Some(ws_client);
        }
        Ok(())
    }

    async fn ws_close(&mut self) -> CcxtResult<()> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.close()?;
            self.ws_client = None;
        }
        Ok(())
    }

    async fn ws_is_connected(&self) -> bool {
        match &self.ws_client {
            Some(c) => c.is_connected().await,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let ws = DeriveWs::default();
        assert!(ws.wallet_address.is_none());
        assert!(ws.private_key.is_none());
    }

    #[test]
    fn test_clone() {
        let ws = DeriveWs::new().with_credentials("0x123", "key123");
        let cloned = ws.clone();
        assert_eq!(cloned.wallet_address, Some("0x123".to_string()));
        assert_eq!(cloned.private_key, Some("key123".to_string()));
    }

    #[test]
    fn test_format_symbol() {
        assert_eq!(DeriveWs::format_symbol("BTC/USD:BTC"), "BTC-PERP");
        assert_eq!(DeriveWs::format_symbol("ETH/USD:ETH"), "ETH-PERP");
        assert_eq!(DeriveWs::format_symbol("BTC/USD"), "BTC-USD");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(DeriveWs::to_unified_symbol("BTC-PERP"), "BTC/USD:BTC");
        assert_eq!(DeriveWs::to_unified_symbol("ETH-PERP"), "ETH/USD:ETH");
        assert_eq!(DeriveWs::to_unified_symbol("BTC-USD"), "BTC/USD");
    }

    #[test]
    fn test_format_interval() {
        assert_eq!(DeriveWs::format_interval(Timeframe::Minute1), "1");
        assert_eq!(DeriveWs::format_interval(Timeframe::Hour1), "60");
        assert_eq!(DeriveWs::format_interval(Timeframe::Day1), "1440");
    }

    #[test]
    fn test_parse_ticker() {
        let ticker_data = DeriveWsTicker {
            instrument_name: Some("BTC-PERP".to_string()),
            best_bid_price: Some("39999.00".to_string()),
            best_bid_amount: Some("1.0".to_string()),
            best_ask_price: Some("40001.00".to_string()),
            best_ask_amount: Some("1.5".to_string()),
            last_price: Some("40000.00".to_string()),
            mark_price: Some("40000.50".to_string()),
            index_price: Some("40000.25".to_string()),
            open_interest: Some("1000".to_string()),
            timestamp: Some(1234567890000),
            stats: Some(DeriveWsTickerStats {
                high: Some("40500.00".to_string()),
                low: Some("39500.00".to_string()),
                volume: Some("1000.0".to_string()),
                volume_usd: Some("40000000.0".to_string()),
                price_change: Some("100.00".to_string()),
            }),
        };

        let ticker = DeriveWs::parse_ticker(&ticker_data, "BTC/USD:BTC");
        assert_eq!(ticker.symbol, "BTC/USD:BTC");
        assert_eq!(ticker.last, Some(Decimal::from(40000)));
        assert_eq!(ticker.bid, Some(Decimal::from(39999)));
        assert_eq!(ticker.ask, Some(Decimal::from(40001)));
    }

    #[test]
    fn test_parse_order_book() {
        let ob_data = DeriveWsOrderBook {
            instrument_name: Some("ETH-PERP".to_string()),
            bids: vec![
                vec!["2500.00".to_string(), "10.0".to_string()],
                vec!["2499.00".to_string(), "5.0".to_string()],
            ],
            asks: vec![
                vec!["2501.00".to_string(), "8.0".to_string()],
                vec!["2502.00".to_string(), "12.0".to_string()],
            ],
            timestamp: Some(1234567890000),
        };

        let order_book = DeriveWs::parse_order_book(&ob_data, "ETH/USD:ETH");
        assert_eq!(order_book.symbol, "ETH/USD:ETH");
        assert_eq!(order_book.bids.len(), 2);
        assert_eq!(order_book.asks.len(), 2);
        assert_eq!(order_book.bids[0].price, Decimal::from(2500));
        assert_eq!(order_book.asks[0].price, Decimal::from(2501));
    }

    #[test]
    fn test_parse_trade() {
        let trade_data = DeriveWsTrade {
            trade_id: Some("123456".to_string()),
            instrument_name: Some("SOL-PERP".to_string()),
            price: Some("100.00".to_string()),
            amount: Some("5.0".to_string()),
            direction: Some("buy".to_string()),
            timestamp: Some(1234567890000),
        };

        let trade = DeriveWs::parse_trade(&trade_data, "SOL/USD:SOL");
        assert_eq!(trade.symbol, "SOL/USD:SOL");
        assert_eq!(trade.id, "123456");
        assert_eq!(trade.price, Decimal::from(100));
        assert_eq!(trade.amount, Decimal::from(5));
        assert_eq!(trade.side, Some("buy".to_string()));
    }

    #[test]
    fn test_parse_ohlcv() {
        let kline_data = DeriveWsKline {
            instrument_name: Some("BTC-PERP".to_string()),
            resolution: Some("60".to_string()),
            open: Some("40000.00".to_string()),
            high: Some("40500.00".to_string()),
            low: Some("39500.00".to_string()),
            close: Some("40200.00".to_string()),
            volume: Some("1000.0".to_string()),
            timestamp: Some(1234567890000),
        };

        let ohlcv = DeriveWs::parse_ohlcv(&kline_data);
        assert!(ohlcv.is_some());
        let ohlcv = ohlcv.unwrap();
        assert_eq!(ohlcv.timestamp, 1234567890000);
        assert_eq!(ohlcv.open, Decimal::from(40000));
        assert_eq!(ohlcv.high, Decimal::from(40500));
        assert_eq!(ohlcv.low, Decimal::from(39500));
        assert_eq!(ohlcv.close, Decimal::from(40200));
        assert_eq!(ohlcv.volume, Decimal::from(1000));
    }

    #[test]
    fn test_parse_balance() {
        let balance_data = DeriveWsBalance {
            currency: Some("USDC".to_string()),
            amount: Some("1100.00".to_string()),
            available: Some("1000.00".to_string()),
            locked: Some("100.00".to_string()),
        };

        let result = DeriveWs::parse_balance(&balance_data);
        assert!(result.is_some());
        let (currency, balance) = result.unwrap();
        assert_eq!(currency, "USDC");
        assert_eq!(balance.free, Some(Decimal::from(1000)));
        assert_eq!(balance.used, Some(Decimal::from(100)));
        assert_eq!(balance.total, Some(Decimal::from(1100)));
    }

    #[test]
    fn test_parse_order() {
        let order_data = DeriveWsOrder {
            order_id: Some("order123".to_string()),
            instrument_name: Some("BTC-PERP".to_string()),
            direction: Some("buy".to_string()),
            order_type: Some("limit".to_string()),
            limit_price: Some("40000.00".to_string()),
            amount: Some("1.0".to_string()),
            filled_amount: Some("0.5".to_string()),
            average_price: Some("40000.00".to_string()),
            order_status: Some("open".to_string()),
            creation_timestamp: Some(1234567890000),
            last_update_timestamp: Some(1234567900000),
        };

        let order = DeriveWs::parse_order(&order_data, "BTC/USD:BTC");
        assert_eq!(order.id, "order123");
        assert_eq!(order.symbol, "BTC/USD:BTC");
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.status, OrderStatus::Open);
        assert_eq!(order.price, Some(Decimal::from(40000)));
    }
}
