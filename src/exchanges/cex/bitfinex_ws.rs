//! Bitfinex WebSocket Implementation
//!
//! Bitfinex 실시간 데이터 스트리밍 (Public & Private Streams)

#![allow(dead_code)]
#![allow(clippy::field_reassign_with_default)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use sha2::Sha384;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::client::{ExchangeConfig, WsClient, WsConfig, WsEvent};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Fee, MarginMode, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, Position, PositionSide, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV, WsBalanceEvent,
    WsExchange, WsMessage, WsOhlcvEvent, WsOrderBookEvent, WsOrderEvent, WsPositionEvent,
    WsTickerEvent, WsTradeEvent,
};

type HmacSha384 = Hmac<Sha384>;

const WS_BASE_URL: &str = "wss://api-pub.bitfinex.com/ws/2";
const WS_AUTH_URL: &str = "wss://api.bitfinex.com/ws/2";

/// Bitfinex WebSocket 클라이언트
pub struct BitfinexWs {
    config: Option<ExchangeConfig>,
    ws_client: Option<WsClient>,
    /// Private WebSocket client (authenticated)
    private_ws_client: Option<WsClient>,
    subscriptions: Arc<RwLock<HashMap<String, String>>>,
    event_tx: Option<mpsc::UnboundedSender<WsMessage>>,
    /// 채널 ID → (채널명, 심볼) 매핑
    channel_map: Arc<RwLock<HashMap<i64, (String, String)>>>,
    /// Authentication status
    authenticated: Arc<RwLock<bool>>,
}

impl BitfinexWs {
    /// 새 Bitfinex WebSocket 클라이언트 생성
    pub fn new() -> Self {
        Self {
            config: None,
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            channel_map: Arc::new(RwLock::new(HashMap::new())),
            authenticated: Arc::new(RwLock::new(false)),
        }
    }

    /// API 키와 시크릿으로 Bitfinex WebSocket 클라이언트 생성
    pub fn with_config(config: ExchangeConfig) -> Self {
        Self {
            config: Some(config),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            channel_map: Arc::new(RwLock::new(HashMap::new())),
            authenticated: Arc::new(RwLock::new(false)),
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

    /// 심볼을 Bitfinex 형식으로 변환 (BTC/USD -> tBTCUSD)
    fn format_symbol(symbol: &str) -> String {
        format!("t{}", symbol.replace("/", ""))
    }

    /// Bitfinex 심볼을 통합 심볼로 변환 (tBTCUSD -> BTC/USD)
    fn to_unified_symbol(bitfinex_symbol: &str) -> String {
        if !bitfinex_symbol.starts_with('t') {
            return bitfinex_symbol.to_string();
        }

        let s = &bitfinex_symbol[1..];

        // Try common quote currencies
        let quotes = ["USDT", "USD", "USDC", "BTC", "ETH", "EUR", "GBP", "JPY"];

        for quote in quotes {
            if let Some(base) = s.strip_suffix(quote) {
                return format!("{base}/{quote}");
            }
        }

        // If no match, try 3/3 split
        if s.len() >= 6 {
            let base = &s[..3];
            let quote = &s[3..];
            return format!("{base}/{quote}");
        }

        bitfinex_symbol.to_string()
    }

    /// Timeframe을 Bitfinex 형식으로 변환
    fn format_timeframe(timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "1m",
            Timeframe::Minute5 => "5m",
            Timeframe::Minute15 => "15m",
            Timeframe::Minute30 => "30m",
            Timeframe::Hour1 => "1h",
            Timeframe::Hour6 => "6h",
            Timeframe::Hour12 => "12h",
            Timeframe::Day1 => "1D",
            Timeframe::Week1 => "1W",
            Timeframe::Month1 => "1M",
            _ => "1m", // Default to 1m for unsupported timeframes
        }
    }

    /// Parse decimal from JSON value (handles both string and number)
    fn parse_decimal(value: &serde_json::Value) -> Option<Decimal> {
        value.as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| value.as_f64().and_then(|f| Decimal::try_from(f).ok()))
    }

    /// 티커 메시지 파싱
    fn parse_ticker(data: &[serde_json::Value], symbol: &str) -> Option<WsTickerEvent> {
        // Ticker format: [BID, BID_SIZE, ASK, ASK_SIZE, DAILY_CHANGE, DAILY_CHANGE_RELATIVE,
        //                 LAST_PRICE, VOLUME, HIGH, LOW]
        if data.len() < 10 {
            return None;
        }

        let bid = Self::parse_decimal(&data[0])?;
        let ask = Self::parse_decimal(&data[2])?;
        let last = Self::parse_decimal(&data[6])?;
        let volume = Self::parse_decimal(&data[7])?;
        let high = Self::parse_decimal(&data[8])?;
        let low = Self::parse_decimal(&data[9])?;
        let change = Self::parse_decimal(&data[4])?;
        let percentage = Self::parse_decimal(&data[5])?;

        let timestamp = Utc::now().timestamp_millis();

        let ticker = Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: Some(high),
            low: Some(low),
            bid: Some(bid),
            bid_volume: Self::parse_decimal(&data[1]),
            ask: Some(ask),
            ask_volume: Self::parse_decimal(&data[3]),
            vwap: None,
            open: None,
            close: Some(last),
            last: Some(last),
            previous_close: None,
            change: Some(change),
            percentage: Some(percentage * Decimal::from(100)),
            average: None,
            base_volume: Some(volume),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        };

        Some(WsTickerEvent {
            symbol: symbol.to_string(),
            ticker,
        })
    }

    /// 호가창 메시지 파싱
    fn parse_order_book(data: &[serde_json::Value], symbol: &str, is_snapshot: bool) -> Option<WsOrderBookEvent> {
        let timestamp = Utc::now().timestamp_millis();
        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if is_snapshot {
            // Snapshot: array of [PRICE, COUNT, AMOUNT]
            for entry in data {
                if let Some(arr) = entry.as_array() {
                    if arr.len() >= 3 {
                        let price = Self::parse_decimal(&arr[0])?;
                        let count: i64 = arr[1].as_i64()?;
                        let amount = Self::parse_decimal(&arr[2])?;

                        if count > 0 {
                            let entry = OrderBookEntry {
                                price,
                                amount: amount.abs(),
                            };

                            if amount > Decimal::ZERO {
                                bids.push(entry);
                            } else {
                                asks.push(entry);
                            }
                        }
                    }
                }
            }
        } else {
            // Update: [PRICE, COUNT, AMOUNT]
            if data.len() >= 3 {
                let price = Self::parse_decimal(&data[0])?;
                let count: i64 = data[1].as_i64()?;
                let amount = Self::parse_decimal(&data[2])?;

                if count > 0 {
                    let entry = OrderBookEntry {
                        price,
                        amount: amount.abs(),
                    };

                    if amount > Decimal::ZERO {
                        bids.push(entry);
                    } else {
                        asks.push(entry);
                    }
                }
            }
        }

        let order_book = OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        };

        Some(WsOrderBookEvent {
            symbol: symbol.to_string(),
            order_book,
            is_snapshot,
        })
    }

    /// 체결 메시지 파싱
    fn parse_trade(data: &[serde_json::Value], symbol: &str) -> Option<WsTradeEvent> {
        // Trade format: [ID, MTS, AMOUNT, PRICE]
        // Or array of trades for snapshot
        let mut trades = Vec::new();

        if data.is_empty() {
            return None;
        }

        // Check if first element is an array (snapshot)
        if data[0].is_array() {
            for trade_data in data {
                if let Some(arr) = trade_data.as_array() {
                    if let Some(trade) = Self::parse_single_trade(arr, symbol) {
                        trades.push(trade);
                    }
                }
            }
        } else {
            // Single trade update
            if let Some(trade) = Self::parse_single_trade(data, symbol) {
                trades.push(trade);
            }
        }

        if trades.is_empty() {
            None
        } else {
            Some(WsTradeEvent {
                symbol: symbol.to_string(),
                trades,
            })
        }
    }

    /// 단일 체결 파싱
    fn parse_single_trade(data: &[serde_json::Value], symbol: &str) -> Option<Trade> {
        if data.len() < 4 {
            return None;
        }

        let id: i64 = data[0].as_i64()?;
        let timestamp: i64 = data[1].as_i64()?;
        let amount = Self::parse_decimal(&data[2])?;
        let price = Self::parse_decimal(&data[3])?;

        let side = if amount > Decimal::ZERO { "buy" } else { "sell" };
        let amount_abs = amount.abs();

        Some(Trade {
            id: id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount: amount_abs,
            cost: Some(price * amount_abs),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Candle 메시지 파싱
    fn parse_candle(data: &[serde_json::Value], symbol: &str, timeframe: Timeframe) -> Option<WsOhlcvEvent> {
        // Candle format: [MTS, OPEN, CLOSE, HIGH, LOW, VOLUME]
        if data.len() < 6 {
            return None;
        }

        let timestamp: i64 = data[0].as_i64()?;
        let open = Self::parse_decimal(&data[1])?;
        let close = Self::parse_decimal(&data[2])?;
        let high = Self::parse_decimal(&data[3])?;
        let low = Self::parse_decimal(&data[4])?;
        let volume = Self::parse_decimal(&data[5])?;

        let ohlcv = OHLCV {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
        };

        Some(WsOhlcvEvent {
            symbol: symbol.to_string(),
            timeframe,
            ohlcv,
        })
    }

    /// Generate authentication signature for WebSocket
    fn generate_auth_signature(api_secret: &str, _nonce: i64, payload: &str) -> CcxtResult<String> {
        let mut mac = HmacSha384::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            })?;
        mac.update(payload.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Subscribe to private stream with authentication
    async fn subscribe_private_stream(&mut self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key and secret required for private streams".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // Create WebSocket connection to authenticated endpoint
        let mut ws_client = WsClient::new(WsConfig {
            url: WS_AUTH_URL.to_string(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;

        // Generate authentication payload
        let nonce = Utc::now().timestamp_millis();
        let auth_payload = format!("AUTH{nonce}");
        let signature = Self::generate_auth_signature(api_secret, nonce, &auth_payload)?;

        // Send authentication message
        let auth_msg = serde_json::json!({
            "event": "auth",
            "apiKey": api_key,
            "authSig": signature,
            "authNonce": nonce,
            "authPayload": auth_payload,
            "filter": ["trading", "wallet", "balance"]
        });

        ws_client.send(&serde_json::to_string(&auth_msg).unwrap_or_default())?;

        self.private_ws_client = Some(ws_client);

        // Process messages
        let tx = event_tx.clone();
        let channel_map = Arc::clone(&self.channel_map);
        let authenticated = Arc::clone(&self.authenticated);

        tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    WsEvent::Connected => {
                        let _ = tx.send(WsMessage::Connected);
                    }
                    WsEvent::Disconnected => {
                        *authenticated.write().await = false;
                        let _ = tx.send(WsMessage::Disconnected);
                    }
                    WsEvent::Message(msg) => {
                        if let Some(ws_msg) = Self::process_private_message(&msg, &authenticated).await {
                            let _ = tx.send(ws_msg);
                        } else if let Some(ws_msg) = Self::process_message(&msg, Arc::clone(&channel_map)) {
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

    /// Process private WebSocket messages
    async fn process_private_message(msg: &str, authenticated: &Arc<RwLock<bool>>) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Handle authentication response
        if let Some(event) = json.get("event").and_then(|v| v.as_str()) {
            match event {
                "auth" => {
                    let status = json.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    if status == "OK" {
                        *authenticated.write().await = true;
                        return Some(WsMessage::Authenticated);
                    } else {
                        let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("Authentication failed");
                        return Some(WsMessage::Error(format!("Auth failed: {msg}")));
                    }
                }
                _ => return None,
            }
        }

        // Handle private data messages [0, "TYPE", DATA]
        if let Some(arr) = json.as_array() {
            if arr.len() >= 3 {
                let chan_id = arr[0].as_i64()?;

                // Private channel has chan_id = 0
                if chan_id != 0 {
                    return None;
                }

                let msg_type = arr[1].as_str()?;

                match msg_type {
                    // Wallet updates
                    "ws" => {
                        // Wallet snapshot: [0, "ws", [[WALLET_TYPE, CURRENCY, BALANCE, ...], ...]]
                        if let Some(wallets) = arr[2].as_array() {
                            return Self::parse_wallet_snapshot(wallets);
                        }
                    }
                    "wu" => {
                        // Wallet update: [0, "wu", [WALLET_TYPE, CURRENCY, BALANCE, ...]]
                        if let Some(wallet) = arr[2].as_array() {
                            return Self::parse_wallet_update(wallet);
                        }
                    }
                    // Order updates
                    "os" => {
                        // Order snapshot: [0, "os", [[ORDER_DATA], ...]]
                        if let Some(orders) = arr[2].as_array() {
                            return Self::parse_order_snapshot(orders);
                        }
                    }
                    "on" | "ou" | "oc" => {
                        // Order new/update/cancel: [0, "on/ou/oc", [ORDER_DATA]]
                        if let Some(order_data) = arr[2].as_array() {
                            return Self::parse_order_update(order_data, msg_type);
                        }
                    }
                    // Position updates
                    "ps" => {
                        // Position snapshot: [0, "ps", [[POSITION_DATA], ...]]
                        if let Some(positions) = arr[2].as_array() {
                            return Self::parse_position_snapshot(positions);
                        }
                    }
                    "pn" | "pu" | "pc" => {
                        // Position new/update/close: [0, "pn/pu/pc", [POSITION_DATA]]
                        if let Some(position_data) = arr[2].as_array() {
                            return Self::parse_position_update(position_data);
                        }
                    }
                    // Trade execution
                    "te" | "tu" => {
                        // Trade executed: [0, "te/tu", [TRADE_DATA]]
                        if let Some(trade_data) = arr[2].as_array() {
                            return Self::parse_my_trade(trade_data);
                        }
                    }
                    "hb" => {
                        // Heartbeat, ignore
                        return None;
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Parse wallet snapshot
    fn parse_wallet_snapshot(wallets: &[serde_json::Value]) -> Option<WsMessage> {
        let mut balances = Balances {
            info: serde_json::to_value(wallets).unwrap_or_default(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
        };

        for wallet in wallets {
            if let Some(arr) = wallet.as_array() {
                if arr.len() >= 3 {
                    let wallet_type = arr[0].as_str().unwrap_or("");
                    let currency = arr[1].as_str().unwrap_or("");
                    let balance = Self::parse_decimal(&arr[2]).unwrap_or(Decimal::ZERO);
                    let available = if arr.len() > 4 {
                        Self::parse_decimal(&arr[4]).unwrap_or(balance)
                    } else {
                        balance
                    };

                    // Only include exchange wallet balances
                    if wallet_type == "exchange" && !currency.is_empty() {
                        let currency_key = currency.to_string();
                        balances.currencies.insert(
                            currency_key,
                            Balance {
                                free: Some(available),
                                used: Some(balance - available),
                                total: Some(balance),
                                debt: None,
                            },
                        );
                    }
                }
            }
        }

        Some(WsMessage::Balance(WsBalanceEvent { balances }))
    }

    /// Parse wallet update
    fn parse_wallet_update(wallet: &[serde_json::Value]) -> Option<WsMessage> {
        if wallet.len() < 3 {
            return None;
        }

        let wallet_type = wallet[0].as_str().unwrap_or("");
        let currency = wallet[1].as_str().unwrap_or("");
        let balance = Self::parse_decimal(&wallet[2]).unwrap_or(Decimal::ZERO);
        let available = if wallet.len() > 4 {
            Self::parse_decimal(&wallet[4]).unwrap_or(balance)
        } else {
            balance
        };

        // Only include exchange wallet updates
        if wallet_type != "exchange" || currency.is_empty() {
            return None;
        }

        let mut balances = Balances {
            info: serde_json::to_value(wallet).unwrap_or_default(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
        };

        let currency_key = currency.to_string();
        balances.currencies.insert(
            currency_key,
            Balance {
                free: Some(available),
                used: Some(balance - available),
                total: Some(balance),
                debt: None,
            },
        );

        Some(WsMessage::Balance(WsBalanceEvent { balances }))
    }

    /// Parse order snapshot
    fn parse_order_snapshot(orders: &[serde_json::Value]) -> Option<WsMessage> {
        for order_data in orders {
            if let Some(arr) = order_data.as_array() {
                if let Some(msg) = Self::parse_order_update(arr, "os") {
                    return Some(msg);
                }
            }
        }
        None
    }

    /// Parse order update
    /// Order format: [ID, GID, CID, SYMBOL, MTS_CREATE, MTS_UPDATE, AMOUNT, AMOUNT_ORIG, TYPE, TYPE_PREV,
    ///                MTS_TIF, _, FLAGS, STATUS, _, _, PRICE, PRICE_AVG, PRICE_TRAILING, PRICE_AUX_LIMIT, ...]
    fn parse_order_update(order_data: &[serde_json::Value], update_type: &str) -> Option<WsMessage> {
        if order_data.len() < 18 {
            return None;
        }

        let order_id = order_data[0].as_i64()?.to_string();
        let symbol_raw = order_data[3].as_str().unwrap_or("");
        let symbol = Self::to_unified_symbol(symbol_raw);
        let mts_update = order_data[5].as_i64().unwrap_or(0);
        let amount = Self::parse_decimal(&order_data[6]).unwrap_or(Decimal::ZERO);
        let amount_orig = Self::parse_decimal(&order_data[7]).unwrap_or(Decimal::ZERO);
        let order_type_str = order_data[8].as_str().unwrap_or("");
        let status_str = order_data[13].as_str().unwrap_or("");
        let price = Self::parse_decimal(&order_data[16]).unwrap_or(Decimal::ZERO);
        let price_avg = Self::parse_decimal(&order_data[17]).unwrap_or(Decimal::ZERO);

        // Determine side from amount (positive = buy, negative = sell)
        let side = if amount_orig > Decimal::ZERO {
            OrderSide::Buy
        } else {
            OrderSide::Sell
        };

        // Parse order type
        let order_type = if order_type_str.contains("LIMIT") {
            OrderType::Limit
        } else if order_type_str.contains("MARKET") {
            OrderType::Market
        } else if order_type_str.contains("STOP") {
            OrderType::StopLoss
        } else {
            OrderType::Limit
        };

        // Parse status
        let status = match status_str {
            s if s.contains("ACTIVE") => OrderStatus::Open,
            s if s.contains("EXECUTED") => OrderStatus::Closed,
            s if s.contains("CANCELED") => OrderStatus::Canceled,
            s if s.contains("PARTIALLY FILLED") => OrderStatus::Open,
            _ => match update_type {
                "on" => OrderStatus::Open,
                "oc" => OrderStatus::Canceled,
                _ => OrderStatus::Open,
            },
        };

        let filled = (amount_orig.abs() - amount.abs()).max(Decimal::ZERO);

        let order = Order {
            id: order_id.clone(),
            client_order_id: Some(order_data[2].as_i64().unwrap_or(0).to_string()),
            timestamp: Some(mts_update),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(mts_update)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: Some(mts_update),
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: if price_avg != Decimal::ZERO { Some(price_avg) } else { None },
            amount: amount_orig.abs(),
            filled,
            remaining: Some(amount.abs()),
            stop_price: None,
            trigger_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            cost: Some(filled * price_avg),
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(order_data).unwrap_or_default(),
        };

        Some(WsMessage::Order(WsOrderEvent { order }))
    }

    /// Parse position snapshot
    fn parse_position_snapshot(positions: &[serde_json::Value]) -> Option<WsMessage> {
        let mut position_list = Vec::new();

        for pos_data in positions {
            if let Some(arr) = pos_data.as_array() {
                if let Some(pos) = Self::parse_single_position(arr) {
                    position_list.push(pos);
                }
            }
        }

        if position_list.is_empty() {
            return None;
        }

        Some(WsMessage::Position(WsPositionEvent {
            positions: position_list,
        }))
    }

    /// Parse position update
    fn parse_position_update(position_data: &[serde_json::Value]) -> Option<WsMessage> {
        let position = Self::parse_single_position(position_data)?;
        Some(WsMessage::Position(WsPositionEvent {
            positions: vec![position],
        }))
    }

    /// Parse single position
    /// Position format: [SYMBOL, STATUS, AMOUNT, BASE_PRICE, FUNDING, FUNDING_TYPE, PL, PL_PERC,
    ///                   PRICE_LIQ, LEVERAGE, FLAG, MTS_CREATE, MTS_UPDATE, ...]
    fn parse_single_position(data: &[serde_json::Value]) -> Option<Position> {
        if data.len() < 13 {
            return None;
        }

        let symbol_raw = data[0].as_str().unwrap_or("");
        let symbol = Self::to_unified_symbol(symbol_raw);
        let status = data[1].as_str().unwrap_or("");
        let amount = Self::parse_decimal(&data[2]).unwrap_or(Decimal::ZERO);
        let base_price = Self::parse_decimal(&data[3]).unwrap_or(Decimal::ZERO);
        let pnl = Self::parse_decimal(&data[6]).unwrap_or(Decimal::ZERO);
        let pnl_percentage = Self::parse_decimal(&data[7]).unwrap_or(Decimal::ZERO);
        let liquidation_price = Self::parse_decimal(&data[8]);
        let leverage = Self::parse_decimal(&data[9]).unwrap_or(Decimal::ONE);
        let mts_update = data[12].as_i64().unwrap_or(0);

        // Skip closed positions
        if status != "ACTIVE" && amount == Decimal::ZERO {
            return None;
        }

        let side = if amount > Decimal::ZERO {
            Some(PositionSide::Long)
        } else {
            Some(PositionSide::Short)
        };

        Some(Position {
            id: None,
            symbol,
            timestamp: Some(mts_update),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(mts_update)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            contracts: Some(amount.abs()),
            contract_size: None,
            side,
            notional: Some(amount.abs() * base_price),
            leverage: Some(leverage),
            unrealized_pnl: Some(pnl),
            realized_pnl: None,
            collateral: None,
            entry_price: Some(base_price),
            mark_price: None,
            last_price: None,
            liquidation_price,
            margin_mode: Some(MarginMode::Cross),
            margin_ratio: None,
            initial_margin: None,
            initial_margin_percentage: None,
            maintenance_margin: None,
            maintenance_margin_percentage: None,
            percentage: Some(pnl_percentage),
            stop_loss_price: None,
            take_profit_price: None,
            hedged: None,
            last_update_timestamp: Some(mts_update),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Parse my trade (executed trade)
    /// Trade format: [ID, SYMBOL, MTS_CREATE, ORDER_ID, EXEC_AMOUNT, EXEC_PRICE, ORDER_TYPE, ORDER_PRICE,
    ///                MAKER, FEE, FEE_CURRENCY, ...]
    fn parse_my_trade(trade_data: &[serde_json::Value]) -> Option<WsMessage> {
        if trade_data.len() < 11 {
            return None;
        }

        let trade_id = trade_data[0].as_i64()?.to_string();
        let symbol_raw = trade_data[1].as_str().unwrap_or("");
        let symbol = Self::to_unified_symbol(symbol_raw);
        let timestamp = trade_data[2].as_i64().unwrap_or(0);
        let order_id = trade_data[3].as_i64().map(|id| id.to_string());
        let exec_amount = Self::parse_decimal(&trade_data[4]).unwrap_or(Decimal::ZERO);
        let exec_price = Self::parse_decimal(&trade_data[5]).unwrap_or(Decimal::ZERO);
        let is_maker = trade_data[8].as_i64().map(|v| v == 1).unwrap_or(false);
        let fee_amount = Self::parse_decimal(&trade_data[9]).unwrap_or(Decimal::ZERO);
        let fee_currency = trade_data[10].as_str().unwrap_or("");

        let side = if exec_amount > Decimal::ZERO {
            "buy"
        } else {
            "sell"
        };

        let trade = Trade {
            id: trade_id,
            order: order_id,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.clone(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: Some(if is_maker { TakerOrMaker::Maker } else { TakerOrMaker::Taker }),
            price: exec_price,
            amount: exec_amount.abs(),
            cost: Some(exec_price * exec_amount.abs()),
            fee: Some(Fee {
                currency: Some(fee_currency.to_string()),
                cost: Some(fee_amount.abs()),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(trade_data).unwrap_or_default(),
        };

        Some(WsMessage::Trade(WsTradeEvent {
            symbol,
            trades: vec![trade],
        }))
    }

    /// 메시지 처리
    fn process_message(msg: &str, channel_map: Arc<RwLock<HashMap<i64, (String, String)>>>) -> Option<WsMessage> {
        let json: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Handle event messages
        if let Some(event) = json.get("event").and_then(|v| v.as_str()) {
            match event {
                "subscribed" => {
                    let chan_id = json.get("chanId")?.as_i64()?;
                    let channel = json.get("channel")?.as_str()?;
                    let symbol = json.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                    let pair = json.get("pair").and_then(|v| v.as_str()).unwrap_or("");
                    let key = json.get("key").and_then(|v| v.as_str()).unwrap_or("");

                    // Store channel mapping
                    let symbol_value = if !symbol.is_empty() {
                        Self::to_unified_symbol(symbol)
                    } else if !pair.is_empty() {
                        Self::to_unified_symbol(&format!("t{pair}"))
                    } else if !key.is_empty() && key.starts_with("trade:") {
                        // Extract symbol from key like "trade:1m:tBTCUSD"
                        if let Some(sym) = key.split(':').nth(2) {
                            Self::to_unified_symbol(sym)
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            channel_map.write().await.insert(chan_id, (channel.to_string(), symbol_value.clone()));
                        })
                    });

                    return Some(WsMessage::Subscribed {
                        channel: channel.to_string(),
                        symbol: if symbol_value.is_empty() { None } else { Some(symbol_value) },
                    });
                }
                "info" | "conf" => {
                    // Informational messages, ignore
                    return None;
                }
                "error" => {
                    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("Unknown error");
                    return Some(WsMessage::Error(msg.to_string()));
                }
                _ => return None,
            }
        }

        // Handle data messages [CHAN_ID, DATA] or [CHAN_ID, "hb"] for heartbeat
        if let Some(arr) = json.as_array() {
            if arr.len() >= 2 {
                let chan_id = arr[0].as_i64()?;

                // Heartbeat
                if arr[1].as_str() == Some("hb") {
                    return None;
                }

                // Get channel info
                let (channel, symbol) = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        channel_map.read().await.get(&chan_id).cloned()
                    })
                })?;

                // Parse based on channel type
                match channel.as_str() {
                    "ticker" => {
                        if let Some(data) = arr[1].as_array() {
                            return Self::parse_ticker(data, &symbol).map(WsMessage::Ticker);
                        }
                    }
                    "book" => {
                        // Check if snapshot or update
                        let is_snapshot = arr[1].is_array() && arr[1].as_array()?.first()?.is_array();

                        if is_snapshot {
                            if let Some(data) = arr[1].as_array() {
                                return Self::parse_order_book(data, &symbol, true).map(WsMessage::OrderBook);
                            }
                        } else if let Some(data) = arr[1].as_array() {
                            return Self::parse_order_book(data, &symbol, false).map(WsMessage::OrderBook);
                        }
                    }
                    "trades" => {
                        if let Some(data_wrapper) = arr[1].as_array() {
                            // Check for snapshot or update
                            if data_wrapper.len() >= 2 {
                                // Update: ["te" or "tu", [trade_data]]
                                if let Some(trade_data) = data_wrapper[1].as_array() {
                                    return Self::parse_trade(trade_data, &symbol).map(WsMessage::Trade);
                                }
                            } else if !data_wrapper.is_empty() {
                                // Snapshot: [[trade1], [trade2], ...]
                                return Self::parse_trade(data_wrapper, &symbol).map(WsMessage::Trade);
                            }
                        }
                    }
                    "candles" => {
                        if let Some(data) = arr[1].as_array() {
                            // Determine timeframe from channel info (stored in symbol field)
                            // For candles, we need to extract timeframe from the key
                            let timeframe = Timeframe::Minute1; // Default, should be stored in channel_map
                            return Self::parse_candle(data, &symbol, timeframe).map(WsMessage::Ohlcv);
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// 구독 시작 및 이벤트 스트림 반환
    async fn subscribe_stream(
        &mut self,
        channel: &str,
        symbol: &str,
        params: Option<serde_json::Value>,
    ) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // WebSocket URL
        let url = WS_BASE_URL.to_string();

        // WebSocket 클라이언트 생성 및 연결
        let mut ws_client = WsClient::new(WsConfig {
            url: url.clone(),
            auto_reconnect: true,
            reconnect_interval_ms: 5000,
            max_reconnect_attempts: 10,
            ping_interval_secs: 30,
            connect_timeout_secs: 30,
        });

        let mut ws_rx = ws_client.connect().await?;
        self.ws_client = Some(ws_client);

        // 구독 저장
        {
            let key = format!("{channel}:{symbol}");
            self.subscriptions.write().await.insert(key, channel.to_string());
        }

        // Send subscription message
        let market_id = Self::format_symbol(symbol);
        let mut sub_msg = serde_json::json!({
            "event": "subscribe",
            "channel": channel,
        });

        if let Some(p) = params {
            sub_msg.as_object_mut().unwrap().extend(p.as_object().unwrap().clone());
        } else {
            // Add symbol for ticker, book, trades
            if channel != "candles" {
                sub_msg["symbol"] = serde_json::json!(market_id);
            }
        }

        if let Some(ws) = &self.ws_client {
            ws.send(&serde_json::to_string(&sub_msg).unwrap_or_default())?;
        }

        // 이벤트 처리 태스크
        let tx = event_tx.clone();
        let channel_map = Arc::clone(&self.channel_map);
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
                        if let Some(ws_msg) = Self::process_message(&msg, Arc::clone(&channel_map)) {
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

impl Default for BitfinexWs {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BitfinexWs {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            ws_client: None,
            private_ws_client: None,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            channel_map: Arc::clone(&self.channel_map),
            authenticated: Arc::new(RwLock::new(false)),
        }
    }
}

#[async_trait]
impl WsExchange for BitfinexWs {
    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("ticker", symbol, None).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);

        // Bitfinex book precision levels: P0 (most precise), P1, P2, P3
        let prec = "P0";
        let len = limit.unwrap_or(25).to_string();

        let params = serde_json::json!({
            "symbol": market_id,
            "prec": prec,
            "len": len,
        });

        client.subscribe_stream("book", symbol, Some(params)).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        client.subscribe_stream("trades", symbol, None).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::new();
        let market_id = Self::format_symbol(symbol);
        let tf = Self::format_timeframe(timeframe);
        let key = format!("trade:{tf}:{market_id}");

        let params = serde_json::json!({
            "key": key,
        });

        client.subscribe_stream("candles", symbol, Some(params)).await
    }

    async fn ws_connect(&mut self) -> CcxtResult<()> {
        // Bitfinex는 구독시 자동 연결
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
        } else if let Some(client) = &self.private_ws_client {
            client.is_connected().await
        } else {
            false
        }
    }

    // === Private Streams ===

    async fn watch_balance(&self) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_config(
            self.config.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key and secret required for private streams".into(),
            })?,
        );
        client.subscribe_private_stream().await
    }

    async fn watch_orders(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_config(
            self.config.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key and secret required for private streams".into(),
            })?,
        );
        client.subscribe_private_stream().await
    }

    async fn watch_my_trades(&self, _symbol: Option<&str>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_config(
            self.config.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key and secret required for private streams".into(),
            })?,
        );
        client.subscribe_private_stream().await
    }

    async fn watch_positions(&self, _symbols: Option<&[&str]>) -> CcxtResult<mpsc::UnboundedReceiver<WsMessage>> {
        let mut client = Self::with_config(
            self.config.clone().ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key and secret required for private streams".into(),
            })?,
        );
        client.subscribe_private_stream().await
    }

    async fn ws_authenticate(&mut self) -> CcxtResult<()> {
        let config = self.config.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key and secret required for authentication".into(),
        })?;

        let api_key = config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        // If already authenticated, return early
        if *self.authenticated.read().await {
            return Ok(());
        }

        // Create WebSocket connection if not exists
        if self.private_ws_client.is_none() {
            let ws_client = WsClient::new(WsConfig {
                url: WS_AUTH_URL.to_string(),
                auto_reconnect: true,
                reconnect_interval_ms: 5000,
                max_reconnect_attempts: 10,
                ping_interval_secs: 30,
                connect_timeout_secs: 30,
            });
            self.private_ws_client = Some(ws_client);
        }

        if let Some(ws_client) = &mut self.private_ws_client {
            // Connect if not connected
            let _ = ws_client.connect().await?;

            // Generate authentication payload
            let nonce = Utc::now().timestamp_millis();
            let auth_payload = format!("AUTH{nonce}");
            let signature = Self::generate_auth_signature(api_secret, nonce, &auth_payload)?;

            // Send authentication message
            let auth_msg = serde_json::json!({
                "event": "auth",
                "apiKey": api_key,
                "authSig": signature,
                "authNonce": nonce,
                "authPayload": auth_payload,
                "filter": ["trading", "wallet", "balance"]
            });

            ws_client.send(&serde_json::to_string(&auth_msg).unwrap_or_default())?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_format_symbol() {
        assert_eq!(BitfinexWs::format_symbol("BTC/USD"), "tBTCUSD");
        assert_eq!(BitfinexWs::format_symbol("ETH/USDT"), "tETHUSDT");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(BitfinexWs::to_unified_symbol("tBTCUSD"), "BTC/USD");
        assert_eq!(BitfinexWs::to_unified_symbol("tETHUSDT"), "ETH/USDT");
        assert_eq!(BitfinexWs::to_unified_symbol("tBTCUSDT"), "BTC/USDT");
    }

    #[test]
    fn test_format_timeframe() {
        assert_eq!(BitfinexWs::format_timeframe(Timeframe::Minute1), "1m");
        assert_eq!(BitfinexWs::format_timeframe(Timeframe::Hour1), "1h");
        assert_eq!(BitfinexWs::format_timeframe(Timeframe::Day1), "1D");
    }

    #[test]
    fn test_with_config() {
        let config = ExchangeConfig::new().with_api_key("key").with_api_secret("secret");
        let ws = BitfinexWs::with_config(config);
        assert!(ws.config.is_some());
        assert!(ws.private_ws_client.is_none());
    }

    #[test]
    fn test_clone() {
        let config = ExchangeConfig::new().with_api_key("key").with_api_secret("secret");
        let original = BitfinexWs::with_config(config);
        let cloned = original.clone();
        assert!(cloned.config.is_some());
        assert!(cloned.ws_client.is_none());
        assert!(cloned.private_ws_client.is_none());
    }

    #[test]
    fn test_parse_wallet_update() {
        let wallet = vec![
            serde_json::json!("exchange"),
            serde_json::json!("BTC"),
            serde_json::json!(1.5),
            serde_json::json!(0),
            serde_json::json!(1.2),
        ];
        let result = BitfinexWs::parse_wallet_update(&wallet);
        assert!(result.is_some());
        if let Some(WsMessage::Balance(event)) = result {
            assert!(event.balances.currencies.contains_key("BTC"));
            let btc = event.balances.currencies.get("BTC").unwrap();
            assert_eq!(btc.total, Some(Decimal::from_str("1.5").unwrap()));
            assert_eq!(btc.free, Some(Decimal::from_str("1.2").unwrap()));
        }
    }

    #[test]
    fn test_parse_order_update() {
        let order = vec![
            serde_json::json!(12345),       // ID
            serde_json::json!(null),        // GID
            serde_json::json!(111),         // CID
            serde_json::json!("tBTCUSD"),   // SYMBOL
            serde_json::json!(1600000000000_i64), // MTS_CREATE
            serde_json::json!(1600000001000_i64), // MTS_UPDATE
            serde_json::json!(0.5),         // AMOUNT
            serde_json::json!(1.0),         // AMOUNT_ORIG
            serde_json::json!("LIMIT"),     // TYPE
            serde_json::json!("LIMIT"),     // TYPE_PREV
            serde_json::json!(null),        // MTS_TIF
            serde_json::json!(null),        // _
            serde_json::json!(0),           // FLAGS
            serde_json::json!("ACTIVE"),    // STATUS
            serde_json::json!(null),        // _
            serde_json::json!(null),        // _
            serde_json::json!(50000.0),     // PRICE
            serde_json::json!(50100.0),     // PRICE_AVG
        ];
        let result = BitfinexWs::parse_order_update(&order, "on");
        assert!(result.is_some());
        if let Some(WsMessage::Order(event)) = result {
            assert_eq!(event.order.id, "12345");
            assert_eq!(event.order.symbol, "BTC/USD");
            assert_eq!(event.order.status, OrderStatus::Open);
            assert_eq!(event.order.side, OrderSide::Buy);
        }
    }

    #[test]
    fn test_parse_position_update() {
        let position = vec![
            serde_json::json!("tBTCUSD"),   // SYMBOL
            serde_json::json!("ACTIVE"),    // STATUS
            serde_json::json!(0.5),         // AMOUNT
            serde_json::json!(48000.0),     // BASE_PRICE
            serde_json::json!(0),           // FUNDING
            serde_json::json!(0),           // FUNDING_TYPE
            serde_json::json!(500.0),       // PL
            serde_json::json!(2.08),        // PL_PERC
            serde_json::json!(45000.0),     // PRICE_LIQ
            serde_json::json!(3.0),         // LEVERAGE
            serde_json::json!(0),           // FLAG
            serde_json::json!(1600000000000_i64), // MTS_CREATE
            serde_json::json!(1600000001000_i64), // MTS_UPDATE
        ];
        let result = BitfinexWs::parse_position_update(&position);
        assert!(result.is_some());
        if let Some(WsMessage::Position(event)) = result {
            assert_eq!(event.positions.len(), 1);
            let pos = &event.positions[0];
            assert_eq!(pos.symbol, "BTC/USD");
            assert_eq!(pos.side, Some(PositionSide::Long));
            assert_eq!(pos.leverage, Some(Decimal::from(3)));
        }
    }

    #[test]
    fn test_parse_my_trade() {
        let trade = vec![
            serde_json::json!(789),         // ID
            serde_json::json!("tBTCUSD"),   // SYMBOL
            serde_json::json!(1600000000000_i64), // MTS_CREATE
            serde_json::json!(12345),       // ORDER_ID
            serde_json::json!(0.1),         // EXEC_AMOUNT
            serde_json::json!(50000.0),     // EXEC_PRICE
            serde_json::json!("LIMIT"),     // ORDER_TYPE
            serde_json::json!(50000.0),     // ORDER_PRICE
            serde_json::json!(1),           // MAKER (1 = true)
            serde_json::json!(-0.0001),     // FEE
            serde_json::json!("BTC"),       // FEE_CURRENCY
        ];
        let result = BitfinexWs::parse_my_trade(&trade);
        assert!(result.is_some());
        if let Some(WsMessage::Trade(event)) = result {
            assert_eq!(event.trades.len(), 1);
            let t = &event.trades[0];
            assert_eq!(t.id, "789");
            assert_eq!(t.symbol, "BTC/USD");
            assert_eq!(t.side, Some("buy".to_string()));
            assert_eq!(t.taker_or_maker, Some(TakerOrMaker::Maker));
        }
    }

    #[test]
    fn test_generate_auth_signature() {
        let signature = BitfinexWs::generate_auth_signature(
            "secret123",
            1600000000000,
            "AUTH1600000000000",
        );
        assert!(signature.is_ok());
        let sig = signature.unwrap();
        assert!(!sig.is_empty());
        assert_eq!(sig.len(), 96); // HMAC-SHA384 produces 48 bytes = 96 hex chars
    }

    #[test]
    fn test_with_credentials() {
        let ws = BitfinexWs::with_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
    }

    #[test]
    fn test_set_credentials() {
        let mut ws = BitfinexWs::new();
        assert!(ws.config.is_none());
        ws.set_credentials("test_key".to_string(), "test_secret".to_string());
        assert!(ws.config.is_some());
    }
}
