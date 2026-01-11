//! Deribit Exchange Implementation
//!
//! Deribit 거래소 API 구현
//! Professional options and futures exchange for BTC and ETH

#![allow(dead_code)]

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance,
    Balances,
    Exchange,
    ExchangeFeatures,
    ExchangeId,
    ExchangeUrls,
    FundingRate,
    FundingRateHistory,
    // Options types
    Greeks,
    MarginMode,
    Market,
    MarketLimits,
    MarketPrecision,
    MarketType,
    MinMax,
    OptionChain,
    OptionContract,
    Order,
    OrderBook,
    OrderBookEntry,
    OrderSide,
    OrderStatus,
    OrderType,
    Position,
    PositionSide,
    SignedRequest,
    Ticker,
    TimeInForce,
    Timeframe,
    Trade,
    OHLCV,
};

const BASE_URL: &str = "https://www.deribit.com";
const TESTNET_URL: &str = "https://test.deribit.com";
const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

type HmacSha256 = Hmac<Sha256>;

/// Deribit exchange structure
pub struct Deribit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    access_token: RwLock<Option<String>>,
    refresh_token: RwLock<Option<String>>,
}

// Response wrappers
#[derive(Debug, Deserialize, Serialize)]
struct DeribitResponse<T> {
    jsonrpc: String,
    id: Option<i64>,
    result: Option<T>,
    error: Option<DeribitError>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitInstrument {
    instrument_name: String,
    kind: String,
    base_currency: String,
    quote_currency: String,
    settlement_currency: Option<String>,
    contract_size: f64,
    creation_timestamp: i64,
    expiration_timestamp: i64,
    is_active: bool,
    settlement_period: Option<String>,
    tick_size: f64,
    min_trade_amount: f64,
    maker_commission: f64,
    taker_commission: f64,
    strike: Option<f64>,
    option_type: Option<String>,
    block_trade_commission: Option<f64>,
    block_trade_min_trade_amount: Option<f64>,
    block_trade_tick_size: Option<f64>,
    price_index: Option<String>,
    rfq: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitTicker {
    instrument_name: String,
    best_bid_price: Option<f64>,
    best_bid_amount: Option<f64>,
    best_ask_price: Option<f64>,
    best_ask_amount: Option<f64>,
    last_price: Option<f64>,
    mark_price: Option<f64>,
    index_price: Option<f64>,
    open_interest: Option<f64>,
    funding_8h: Option<f64>,
    current_funding: Option<f64>,
    estimated_delivery_price: Option<f64>,
    settlement_price: Option<f64>,
    stats: Option<DeribitStats>,
    state: Option<String>,
    timestamp: i64,
    greeks: Option<DeribitGreeks>,
    underlying_price: Option<f64>,
    underlying_index: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitStats {
    high: Option<f64>,
    low: Option<f64>,
    volume: Option<f64>,
    volume_usd: Option<f64>,
    price_change: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitGreeks {
    delta: Option<f64>,
    gamma: Option<f64>,
    vega: Option<f64>,
    theta: Option<f64>,
    rho: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOrderBook {
    instrument_name: String,
    timestamp: i64,
    bids: Vec<Vec<f64>>,
    asks: Vec<Vec<f64>>,
    change_id: Option<i64>,
    state: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitTrade {
    trade_id: String,
    instrument_name: String,
    timestamp: i64,
    direction: String,
    price: f64,
    amount: f64,
    mark_price: Option<f64>,
    index_price: Option<f64>,
    trade_seq: Option<i64>,
    tick_direction: Option<i32>,
    iv: Option<f64>,
    liquidation: Option<String>,
    block_trade_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOhlcv {
    tick: i64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    cost: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitAuth {
    access_token: String,
    expires_in: i64,
    refresh_token: String,
    scope: String,
    token_type: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitAccountSummary {
    currency: String,
    balance: f64,
    equity: f64,
    available_funds: f64,
    available_withdrawal_funds: f64,
    initial_margin: f64,
    maintenance_margin: f64,
    margin_balance: f64,
    portfolio_margining_enabled: Option<bool>,
    total_pl: Option<f64>,
    delta_total: Option<f64>,
    options_pl: Option<f64>,
    options_session_upl: Option<f64>,
    options_delta: Option<f64>,
    options_gamma: Option<f64>,
    options_theta: Option<f64>,
    options_vega: Option<f64>,
    futures_pl: Option<f64>,
    futures_session_upl: Option<f64>,
    session_upl: Option<f64>,
    session_rpl: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitOrder {
    order_id: String,
    instrument_name: String,
    order_state: String,
    direction: String,
    order_type: String,
    time_in_force: String,
    price: Option<f64>,
    amount: f64,
    filled_amount: f64,
    average_price: Option<f64>,
    creation_timestamp: i64,
    last_update_timestamp: i64,
    label: Option<String>,
    trigger_price: Option<f64>,
    trigger: Option<String>,
    stop_price: Option<f64>,
    reduce_only: bool,
    post_only: bool,
    reject_post_only: Option<bool>,
    advanced: Option<String>,
    mmp: Option<bool>,
    usd: Option<f64>,
    implv: Option<f64>,
    replaced: Option<bool>,
    max_show: Option<f64>,
    api: Option<bool>,
    profit_loss: Option<f64>,
    commission: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitPosition {
    instrument_name: String,
    direction: String,
    size: f64,
    average_price: f64,
    average_price_usd: Option<f64>,
    floating_profit_loss: f64,
    floating_profit_loss_usd: Option<f64>,
    realized_profit_loss: f64,
    total_profit_loss: f64,
    delta: Option<f64>,
    gamma: Option<f64>,
    theta: Option<f64>,
    vega: Option<f64>,
    mark_price: f64,
    index_price: Option<f64>,
    initial_margin: f64,
    maintenance_margin: f64,
    open_orders_margin: f64,
    leverage: Option<i32>,
    settlement_price: Option<f64>,
    estimated_liquidation_price: Option<f64>,
    kind: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct DeribitFundingRate {
    instrument_name: String,
    timestamp: i64,
    interest_8h: f64,
    interest_1h: f64,
}

impl Deribit {
    /// Create a new Deribit exchange instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let sandbox = config.is_sandbox();
        let base_url = if sandbox { TESTNET_URL } else { BASE_URL };
        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: false,
            margin: false,
            swap: true,
            future: true,
            option: true,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_positions: true,
            set_leverage: true,
            fetch_leverage: true,
            fetch_funding_rate: true,
            fetch_funding_rates: true,
            fetch_funding_rate_history: true,
            ws: true,
            watch_ticker: true,
            watch_order_book: true,
            watch_trades: true,
            // Options trading features
            fetch_option: true,
            fetch_option_chain: true,
            fetch_greeks: true,
            fetch_underlying_assets: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".to_string(), format!("{base_url}/api/v2"));
        api_urls.insert("private".to_string(), format!("{base_url}/api/v2"));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/41933112-9e2dd65a-798b-11e8-8440-5aab2959fcb5.jpg".to_string()),
            api: api_urls,
            www: Some("https://www.deribit.com".to_string()),
            doc: vec!["https://docs.deribit.com/v2".to_string()],
            fees: Some("https://www.deribit.com/pages/information/fees".to_string()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute3, "3".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour2, "120".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Hour6, "360".into());
        timeframes.insert(Timeframe::Hour12, "720".into());
        timeframes.insert(Timeframe::Day1, "1D".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            access_token: RwLock::new(None),
            refresh_token: RwLock::new(None),
        })
    }

    /// Generate HMAC-SHA256 signature
    fn sign(&self, message: &str) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".to_string(),
            })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| {
            CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            }
        })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Authenticate and get access token
    async fn authenticate(&self) -> CcxtResult<String> {
        // Check if we have a valid token
        {
            let token = self.access_token.read().unwrap();
            if let Some(t) = token.as_ref() {
                return Ok(t.clone());
            }
        }

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".to_string(),
            })?;

        let params = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "public/auth",
            "params": {
                "grant_type": "client_credentials",
                "client_id": api_key,
                "client_secret": secret
            }
        });

        let response: DeribitResponse<DeribitAuth> = self
            .client
            .post("/api/v2/public/auth", Some(params), None)
            .await?;

        if let Some(error) = response.error {
            return Err(CcxtError::AuthenticationError {
                message: format!("{}: {}", error.code, error.message),
            });
        }

        let auth = response
            .result
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "No auth result".to_string(),
            })?;

        *self.access_token.write().unwrap() = Some(auth.access_token.clone());
        *self.refresh_token.write().unwrap() = Some(auth.refresh_token);

        Ok(auth.access_token)
    }

    /// Make authenticated JSON-RPC request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> CcxtResult<T> {
        let token = self.authenticate().await?;
        self.rate_limiter.throttle(1.0).await;

        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {token}"));
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.unwrap_or(json!({}))
        });

        let path = format!("/api/v2/{method}");
        let response: DeribitResponse<T> =
            self.client.post(&path, Some(body), Some(headers)).await?;

        if let Some(error) = response.error {
            return Err(CcxtError::ExchangeError {
                message: format!("{}: {}", error.code, error.message),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "No result in response".to_string(),
        })
    }

    /// Make public JSON-RPC request
    async fn public_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.unwrap_or(json!({}))
        });

        let path = format!("/api/v2/{method}");
        let response: DeribitResponse<T> = self.client.post(&path, Some(body), None).await?;

        if let Some(error) = response.error {
            return Err(CcxtError::ExchangeError {
                message: format!("{}: {}", error.code, error.message),
            });
        }

        response.result.ok_or_else(|| CcxtError::ExchangeError {
            message: "No result in response".to_string(),
        })
    }

    /// Parse order status
    fn parse_order_status(status: &str) -> OrderStatus {
        match status {
            "open" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "cancelled" | "canceled" => OrderStatus::Canceled,
            "rejected" => OrderStatus::Rejected,
            "untriggered" => OrderStatus::Open,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order side
    fn parse_order_side(direction: &str) -> OrderSide {
        match direction.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order type
    fn parse_order_type(order_type: &str) -> OrderType {
        match order_type {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            "stop_limit" => OrderType::StopLimit,
            "stop_market" => OrderType::StopMarket,
            "take_limit" => OrderType::TakeProfitLimit,
            "take_market" => OrderType::TakeProfitMarket,
            _ => OrderType::Limit,
        }
    }

    /// Parse time in force
    fn parse_time_in_force(tif: &str) -> TimeInForce {
        match tif {
            "good_til_cancelled" | "gtc" => TimeInForce::GTC,
            "fill_or_kill" | "fok" => TimeInForce::FOK,
            "immediate_or_cancel" | "ioc" => TimeInForce::IOC,
            _ => TimeInForce::GTC,
        }
    }

    /// Convert order to unified format
    fn parse_order(&self, order: &DeribitOrder) -> Order {
        let timestamp = Some(order.creation_timestamp);
        let datetime = chrono::DateTime::from_timestamp_millis(order.creation_timestamp)
            .map(|dt| dt.to_rfc3339());

        let side = Self::parse_order_side(&order.direction);
        let status = Self::parse_order_status(&order.order_state);
        let order_type = Self::parse_order_type(&order.order_type);
        let time_in_force = Self::parse_time_in_force(&order.time_in_force);

        let amount = Decimal::from_f64_retain(order.amount).unwrap_or_default();
        let filled = Decimal::from_f64_retain(order.filled_amount).unwrap_or_default();
        let remaining = amount - filled;
        let price = order
            .price
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let average = order
            .average_price
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let stop_price = order
            .stop_price
            .or(order.trigger_price)
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());

        let cost = average.map(|avg| avg * filled);

        Order {
            id: order.order_id.clone(),
            client_order_id: order.label.clone(),
            timestamp,
            datetime,
            last_trade_timestamp: Some(order.last_update_timestamp),
            last_update_timestamp: Some(order.last_update_timestamp),
            status,
            symbol: order.instrument_name.clone(),
            order_type,
            time_in_force: Some(time_in_force),
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            reduce_only: Some(order.reduce_only),
            post_only: Some(order.post_only),
            stop_price,
            trigger_price: stop_price,
            take_profit_price: None,
            stop_loss_price: None,
            trades: Vec::new(),
            fee: order.commission.map(|c| crate::types::Fee {
                currency: Some("USD".to_string()),
                cost: Some(Decimal::from_f64_retain(c).unwrap_or_default()),
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(order).unwrap_or(Value::Null),
        }
    }

    /// Convert position to unified format
    fn parse_position(&self, pos: &DeribitPosition) -> Position {
        let timestamp = Some(chrono::Utc::now().timestamp_millis());
        let datetime = Some(chrono::Utc::now().to_rfc3339());

        let side = if pos.size > 0.0 {
            Some(PositionSide::Long)
        } else if pos.size < 0.0 {
            Some(PositionSide::Short)
        } else {
            None
        };

        let contracts = Some(Decimal::from_f64_retain(pos.size.abs()).unwrap_or_default());
        let entry_price = Some(Decimal::from_f64_retain(pos.average_price).unwrap_or_default());
        let mark_price = Some(Decimal::from_f64_retain(pos.mark_price).unwrap_or_default());
        let liquidation_price = pos
            .estimated_liquidation_price
            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let leverage = pos.leverage.map(Decimal::from);
        let unrealized_pnl =
            Some(Decimal::from_f64_retain(pos.floating_profit_loss).unwrap_or_default());
        let realized_pnl =
            Some(Decimal::from_f64_retain(pos.realized_profit_loss).unwrap_or_default());
        let initial_margin = Some(Decimal::from_f64_retain(pos.initial_margin).unwrap_or_default());
        let maintenance_margin =
            Some(Decimal::from_f64_retain(pos.maintenance_margin).unwrap_or_default());

        Position {
            symbol: pos.instrument_name.clone(),
            id: None,
            info: serde_json::to_value(pos).unwrap_or(Value::Null),
            timestamp,
            datetime,
            contracts,
            contract_size: Some(Decimal::ONE),
            side,
            notional: None,
            leverage,
            unrealized_pnl,
            realized_pnl,
            collateral: None,
            entry_price,
            mark_price,
            liquidation_price,
            margin_mode: Some(MarginMode::Cross),
            hedged: Some(false),
            maintenance_margin,
            maintenance_margin_percentage: None,
            initial_margin,
            initial_margin_percentage: None,
            margin_ratio: None,
            last_update_timestamp: None,
            last_price: Some(Decimal::from_f64_retain(pos.mark_price).unwrap_or_default()),
            stop_loss_price: None,
            take_profit_price: None,
            percentage: None,
        }
    }

    /// Parse unified symbol from Deribit instrument name
    fn parse_symbol(&self, instrument_name: &str) -> String {
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
}

#[async_trait]
impl Exchange for Deribit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Deribit
    }

    fn name(&self) -> &'static str {
        "Deribit"
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let mut all_markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        // Fetch instruments for all currencies
        for currency in &["BTC", "ETH", "SOL", "USDC"] {
            for kind in &["future", "option"] {
                let params = json!({
                    "currency": currency,
                    "kind": kind,
                    "expired": false
                });

                let instruments: Vec<DeribitInstrument> = match self
                    .public_request("public/get_instruments", Some(params))
                    .await
                {
                    Ok(i) => i,
                    Err(_) => continue,
                };

                for inst in instruments {
                    let symbol = self.parse_symbol(&inst.instrument_name);
                    let is_perpetual = inst.instrument_name.contains("PERPETUAL");
                    let is_option = inst.kind == "option";

                    let market_type = if is_option {
                        MarketType::Option
                    } else if is_perpetual {
                        MarketType::Swap
                    } else {
                        MarketType::Future
                    };

                    let precision = MarketPrecision {
                        price: Some((1.0 / inst.tick_size).log10().ceil() as i32),
                        amount: Some((1.0 / inst.min_trade_amount).log10().ceil() as i32),
                        cost: None,
                        base: None,
                        quote: None,
                    };

                    let limits = MarketLimits {
                        amount: MinMax {
                            min: Some(
                                Decimal::from_f64_retain(inst.min_trade_amount).unwrap_or_default(),
                            ),
                            max: None,
                        },
                        price: MinMax {
                            min: Some(Decimal::from_f64_retain(inst.tick_size).unwrap_or_default()),
                            max: None,
                        },
                        cost: MinMax {
                            min: None,
                            max: None,
                        },
                        leverage: MinMax {
                            min: Some(Decimal::ONE),
                            max: Some(Decimal::from(100)),
                        },
                    };

                    let expiry = if inst.expiration_timestamp > 0 && !is_perpetual {
                        Some(inst.expiration_timestamp)
                    } else {
                        None
                    };

                    let market = Market {
                        id: inst.instrument_name.clone(),
                        lowercase_id: Some(inst.instrument_name.to_lowercase()),
                        symbol: symbol.clone(),
                        base: inst.base_currency.clone(),
                        quote: inst.quote_currency.clone(),
                        base_id: inst.base_currency.clone(),
                        quote_id: inst.quote_currency.clone(),
                        settle_id: inst.settlement_currency.clone(),
                        active: inst.is_active,
                        market_type,
                        spot: false,
                        margin: false,
                        swap: is_perpetual,
                        future: !is_perpetual && !is_option,
                        option: is_option,
                        index: false,
                        contract: true,
                        settle: inst.settlement_currency.clone(),
                        contract_size: Some(
                            Decimal::from_f64_retain(inst.contract_size).unwrap_or(Decimal::ONE),
                        ),
                        linear: Some(false),
                        inverse: Some(true),
                        sub_type: None,
                        expiry,
                        expiry_datetime: expiry.map(|e| {
                            chrono::DateTime::from_timestamp_millis(e)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default()
                        }),
                        strike: inst
                            .strike
                            .map(|s| Decimal::from_f64_retain(s).unwrap_or_default()),
                        option_type: inst.option_type.clone(),
                        taker: Some(
                            Decimal::from_f64_retain(inst.taker_commission).unwrap_or_default(),
                        ),
                        maker: Some(
                            Decimal::from_f64_retain(inst.maker_commission).unwrap_or_default(),
                        ),
                        percentage: true,
                        tier_based: false,
                        precision,
                        limits,
                        margin_modes: None,
                        created: Some(inst.creation_timestamp),
                        info: serde_json::to_value(&inst).unwrap_or(Value::Null),
                    };

                    markets_by_id.insert(inst.instrument_name.clone(), symbol.clone());
                    all_markets.insert(symbol, market);
                }
            }
        }

        *self.markets.write().unwrap() = all_markets.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(all_markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(true).await?;
        Ok(markets.values().cloned().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let params = json!({ "instrument_name": market_id });
        let ticker: DeribitTicker = self.public_request("public/ticker", Some(params)).await?;

        let timestamp = Some(ticker.timestamp);
        let datetime =
            chrono::DateTime::from_timestamp_millis(ticker.timestamp).map(|dt| dt.to_rfc3339());

        let (high, low, volume, percentage) = if let Some(stats) = &ticker.stats {
            (
                stats
                    .high
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                stats
                    .low
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                stats
                    .volume
                    .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
                stats
                    .price_change
                    .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            )
        } else {
            (None, None, None, None)
        };

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime,
            high,
            low,
            bid: ticker
                .best_bid_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid_volume: ticker
                .best_bid_amount
                .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
            ask: ticker
                .best_ask_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            ask_volume: ticker
                .best_ask_amount
                .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
            vwap: None,
            open: None,
            close: ticker
                .last_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            last: ticker
                .last_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            previous_close: None,
            change: None,
            percentage,
            average: None,
            base_volume: volume,
            quote_volume: ticker
                .stats
                .as_ref()
                .and_then(|s| s.volume_usd)
                .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
            index_price: ticker
                .index_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            mark_price: ticker
                .mark_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            info: serde_json::to_value(&ticker).unwrap_or(Value::Null),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let mut result = HashMap::new();

        // Fetch book summaries for each currency
        for currency in &["BTC", "ETH"] {
            let params = json!({ "currency": currency, "kind": "future" });
            let summaries: Vec<DeribitTicker> = match self
                .public_request("public/get_book_summary_by_currency", Some(params))
                .await
            {
                Ok(s) => s,
                Err(_) => continue,
            };

            let markets_by_id = self.markets_by_id.read().unwrap();

            for ticker in summaries {
                if let Some(symbol) = markets_by_id.get(&ticker.instrument_name) {
                    if let Some(filter_symbols) = symbols {
                        if !filter_symbols.contains(&symbol.as_str()) {
                            continue;
                        }
                    }

                    let timestamp = Some(ticker.timestamp);
                    let datetime = chrono::DateTime::from_timestamp_millis(ticker.timestamp)
                        .map(|dt| dt.to_rfc3339());

                    let (high, low, volume, percentage) = if let Some(stats) = &ticker.stats {
                        (
                            stats
                                .high
                                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                            stats
                                .low
                                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                            stats
                                .volume
                                .map(|v| Decimal::from_f64_retain(v).unwrap_or_default()),
                            stats
                                .price_change
                                .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        )
                    } else {
                        (None, None, None, None)
                    };

                    let t = Ticker {
                        symbol: symbol.clone(),
                        timestamp,
                        datetime,
                        high,
                        low,
                        bid: ticker
                            .best_bid_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        bid_volume: ticker
                            .best_bid_amount
                            .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
                        ask: ticker
                            .best_ask_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        ask_volume: ticker
                            .best_ask_amount
                            .map(|a| Decimal::from_f64_retain(a).unwrap_or_default()),
                        vwap: None,
                        open: None,
                        close: ticker
                            .last_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        last: ticker
                            .last_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        previous_close: None,
                        change: None,
                        percentage,
                        average: None,
                        base_volume: volume,
                        quote_volume: None,
                        index_price: ticker
                            .index_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        mark_price: ticker
                            .mark_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        info: serde_json::to_value(&ticker).unwrap_or(Value::Null),
                    };

                    result.insert(symbol.clone(), t);
                }
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let depth = limit.unwrap_or(20);
        let params = json!({
            "instrument_name": market_id,
            "depth": depth
        });

        let book: DeribitOrderBook = self
            .public_request("public/get_order_book", Some(params))
            .await?;

        let bids: Vec<OrderBookEntry> = book
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: Decimal::from_f64_retain(b[0]).unwrap_or_default(),
                amount: Decimal::from_f64_retain(b[1]).unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = book
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: Decimal::from_f64_retain(a[0]).unwrap_or_default(),
                amount: Decimal::from_f64_retain(a[1]).unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(book.timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(book.timestamp)
                .map(|dt| dt.to_rfc3339()),
            nonce: book.change_id,
            bids,
            asks,
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let params = json!({
            "instrument_name": market_id,
            "count": limit.unwrap_or(100)
        });

        #[derive(Debug, Deserialize)]
        struct TradesResponse {
            trades: Vec<DeribitTrade>,
        }

        let response: TradesResponse = self
            .public_request("public/get_last_trades_by_instrument", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .trades
            .iter()
            .map(|t| {
                let timestamp = Some(t.timestamp);
                let datetime =
                    chrono::DateTime::from_timestamp_millis(t.timestamp).map(|dt| dt.to_rfc3339());

                let side = match t.direction.to_lowercase().as_str() {
                    "buy" => "buy",
                    "sell" => "sell",
                    _ => "buy",
                };

                let price = Decimal::from_f64_retain(t.price).unwrap_or_default();
                let amount = Decimal::from_f64_retain(t.amount).unwrap_or_default();

                Trade {
                    id: t.trade_id.clone(),
                    timestamp,
                    datetime,
                    symbol: symbol.to_string(),
                    order: None,
                    trade_type: None,
                    side: Some(side.to_string()),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or(Value::Null),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let resolution = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "60".to_string());

        let now = chrono::Utc::now().timestamp_millis();
        let start = since.unwrap_or(now - 86400000); // Default to last 24 hours
        let end = now;

        let params = json!({
            "instrument_name": market_id,
            "resolution": resolution,
            "start_timestamp": start,
            "end_timestamp": end
        });

        #[derive(Debug, Deserialize)]
        struct OhlcvResponse {
            ticks: Vec<i64>,
            open: Vec<f64>,
            high: Vec<f64>,
            low: Vec<f64>,
            close: Vec<f64>,
            volume: Vec<f64>,
        }

        let response: OhlcvResponse = self
            .public_request("public/get_tradingview_chart_data", Some(params))
            .await?;

        let count = response.ticks.len();
        let max_count = limit.unwrap_or(500) as usize;

        let mut result: Vec<OHLCV> = (0..count.min(max_count))
            .map(|i| OHLCV {
                timestamp: response.ticks[i],
                open: Decimal::from_f64_retain(response.open[i]).unwrap_or_default(),
                high: Decimal::from_f64_retain(response.high[i]).unwrap_or_default(),
                low: Decimal::from_f64_retain(response.low[i]).unwrap_or_default(),
                close: Decimal::from_f64_retain(response.close[i]).unwrap_or_default(),
                volume: Decimal::from_f64_retain(response.volume[i]).unwrap_or_default(),
            })
            .collect();

        result.sort_by_key(|c| c.timestamp);

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let mut result = Balances::new();

        for currency in &["BTC", "ETH"] {
            let params = json!({ "currency": currency });
            let account: DeribitAccountSummary = match self
                .private_request("private/get_account_summary", Some(params))
                .await
            {
                Ok(a) => a,
                Err(_) => continue,
            };

            result.currencies.insert(
                account.currency.clone(),
                Balance {
                    free: Some(
                        Decimal::from_f64_retain(account.available_funds).unwrap_or_default(),
                    ),
                    used: Some(
                        Decimal::from_f64_retain(account.initial_margin).unwrap_or_default(),
                    ),
                    total: Some(Decimal::from_f64_retain(account.equity).unwrap_or_default()),
                    debt: None,
                },
            );
        }

        Ok(result)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let _params = if let Some(sym) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets
                    .get(sym)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: sym.to_string(),
                    })?
            };
            json!({ "instrument_name": market_id })
        } else {
            json!({})
        };

        let orders: Vec<DeribitOrder> = self
            .private_request(
                "private/get_open_orders_by_currency",
                Some(json!({ "currency": "any" })),
            )
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let result: Vec<Order> = orders
            .iter()
            .filter(|o| {
                if let Some(sym) = symbol {
                    if let Some(order_symbol) = markets_by_id.get(&o.instrument_name) {
                        order_symbol == sym
                    } else {
                        false
                    }
                } else {
                    true
                }
            })
            .map(|o| {
                let mut order = self.parse_order(o);
                if let Some(s) = markets_by_id.get(&o.instrument_name) {
                    order.symbol = s.clone();
                }
                order
            })
            .collect();

        Ok(result)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let params = json!({ "order_id": id });
        let order: DeribitOrder = self
            .private_request("private/get_order_state", Some(params))
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut result = self.parse_order(&order);
        if let Some(s) = markets_by_id.get(&order.instrument_name) {
            result.symbol = s.clone();
        }

        Ok(result)
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        self.load_markets(false).await?;

        let mut all_positions = Vec::new();

        for currency in &["BTC", "ETH"] {
            let params = json!({ "currency": currency });
            let positions: Vec<DeribitPosition> = match self
                .private_request("private/get_positions", Some(params))
                .await
            {
                Ok(p) => p,
                Err(_) => continue,
            };

            let markets_by_id = self.markets_by_id.read().unwrap();

            for pos in positions {
                if pos.size.abs() < 0.0001 {
                    continue;
                }

                if let Some(symbol) = markets_by_id.get(&pos.instrument_name) {
                    if let Some(filter_symbols) = symbols {
                        if !filter_symbols.contains(&symbol.as_str()) {
                            continue;
                        }
                    }

                    let mut position = self.parse_position(&pos);
                    position.symbol = symbol.clone();
                    all_positions.push(position);
                }
            }
        }

        Ok(all_positions)
    }

    async fn fetch_funding_rates(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, FundingRate>> {
        self.load_markets(false).await?;

        let mut result = HashMap::new();

        for currency in &["BTC", "ETH"] {
            let perpetual = format!("{currency}-PERPETUAL");
            let params = json!({ "instrument_name": perpetual });

            let ticker: DeribitTicker =
                match self.public_request("public/ticker", Some(params)).await {
                    Ok(t) => t,
                    Err(_) => continue,
                };

            let markets_by_id = self.markets_by_id.read().unwrap();
            if let Some(symbol) = markets_by_id.get(&ticker.instrument_name) {
                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = Some(ticker.timestamp);
                let datetime = chrono::DateTime::from_timestamp_millis(ticker.timestamp)
                    .map(|dt| dt.to_rfc3339());

                result.insert(
                    symbol.clone(),
                    FundingRate {
                        info: serde_json::to_value(&ticker).unwrap_or(Value::Null),
                        symbol: symbol.clone(),
                        mark_price: ticker
                            .mark_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        index_price: ticker
                            .index_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        interest_rate: None,
                        estimated_settle_price: ticker
                            .estimated_delivery_price
                            .map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        timestamp,
                        datetime,
                        funding_rate: ticker
                            .current_funding
                            .map(|f| Decimal::from_f64_retain(f).unwrap_or_default()),
                        funding_timestamp: None,
                        funding_datetime: None,
                        next_funding_rate: ticker
                            .funding_8h
                            .map(|f| Decimal::from_f64_retain(f).unwrap_or_default()),
                        next_funding_timestamp: None,
                        next_funding_datetime: None,
                        previous_funding_rate: None,
                        previous_funding_timestamp: None,
                        previous_funding_datetime: None,
                        interval: Some("8h".to_string()),
                    },
                );
            }
        }

        Ok(result)
    }

    async fn fetch_funding_rate_history(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<FundingRateHistory>> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let now = chrono::Utc::now().timestamp_millis();
        let start = since.unwrap_or(now - 7 * 86400000); // Default to last 7 days
        let end = now;

        let params = json!({
            "instrument_name": market_id,
            "start_timestamp": start,
            "end_timestamp": end
        });

        let funding: Vec<DeribitFundingRate> = self
            .public_request("public/get_funding_rate_history", Some(params))
            .await?;

        let max_count = limit.unwrap_or(100) as usize;
        let result: Vec<FundingRateHistory> = funding
            .iter()
            .take(max_count)
            .map(|f| {
                let timestamp = Some(f.timestamp);
                let datetime =
                    chrono::DateTime::from_timestamp_millis(f.timestamp).map(|dt| dt.to_rfc3339());

                FundingRateHistory {
                    info: serde_json::to_value(f).unwrap_or(Value::Null),
                    symbol: symbol.to_string(),
                    funding_rate: Decimal::from_f64_retain(f.interest_8h).unwrap_or_default(),
                    timestamp,
                    datetime,
                }
            })
            .collect();

        Ok(result)
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets
                .get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let method = match side {
            OrderSide::Buy => "private/buy",
            OrderSide::Sell => "private/sell",
        };

        let type_str = match order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::StopMarket => "stop_market",
            OrderType::StopLimit => "stop_limit",
            _ => "limit",
        };

        let mut params = json!({
            "instrument_name": market_id,
            "amount": amount.to_string().parse::<f64>().unwrap_or(0.0),
            "type": type_str
        });

        if let Some(p) = price {
            params["price"] = json!(p.to_string().parse::<f64>().unwrap_or(0.0));
        }

        #[derive(Debug, Deserialize)]
        struct OrderResponse {
            order: DeribitOrder,
        }

        let response: OrderResponse = self.private_request(method, Some(params)).await?;

        let mut result = self.parse_order(&response.order);
        result.symbol = symbol.to_string();

        Ok(result)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let params = json!({ "order_id": id });
        let order: DeribitOrder = self.private_request("private/cancel", Some(params)).await?;

        Ok(self.parse_order(&order))
    }

    fn has(&self) -> &ExchangeFeatures {
        &self.features
    }

    fn urls(&self) -> &ExchangeUrls {
        &self.urls
    }

    fn timeframes(&self) -> &HashMap<Timeframe, String> {
        &self.timeframes
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let markets = self.markets.read().unwrap();
        markets.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    // === Options Trading Implementation ===

    async fn fetch_option(&self, symbol: &str) -> CcxtResult<OptionContract> {
        let instrument_name = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let params = json!({ "instrument_name": instrument_name });
        let ticker: DeribitTicker = self.public_request("public/ticker", Some(params)).await?;

        Ok(OptionContract {
            info: serde_json::to_value(&ticker).unwrap_or_default(),
            currency: ticker.underlying_index.clone().unwrap_or_default(),
            symbol: symbol.to_string(),
            timestamp: Some(ticker.timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(ticker.timestamp)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
            implied_volatility: Decimal::try_from(
                ticker.greeks.as_ref().map(|_| 0.0).unwrap_or(0.0),
            )
            .unwrap_or_default(),
            open_interest: Decimal::try_from(ticker.open_interest.unwrap_or(0.0))
                .unwrap_or_default(),
            bid_price: Decimal::try_from(ticker.best_bid_price.unwrap_or(0.0)).unwrap_or_default(),
            ask_price: Decimal::try_from(ticker.best_ask_price.unwrap_or(0.0)).unwrap_or_default(),
            mid_price: Decimal::try_from(
                (ticker.best_bid_price.unwrap_or(0.0) + ticker.best_ask_price.unwrap_or(0.0)) / 2.0,
            )
            .unwrap_or_default(),
            mark_price: Decimal::try_from(ticker.mark_price.unwrap_or(0.0)).unwrap_or_default(),
            last_price: Decimal::try_from(ticker.last_price.unwrap_or(0.0)).unwrap_or_default(),
            underlying_price: Decimal::try_from(ticker.underlying_price.unwrap_or(0.0))
                .unwrap_or_default(),
            change: Decimal::try_from(
                ticker
                    .stats
                    .as_ref()
                    .and_then(|s| s.price_change)
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
            percentage: Decimal::try_from(
                ticker
                    .stats
                    .as_ref()
                    .and_then(|s| s.price_change)
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
            base_volume: Decimal::try_from(
                ticker.stats.as_ref().and_then(|s| s.volume).unwrap_or(0.0),
            )
            .unwrap_or_default(),
            quote_volume: Decimal::try_from(
                ticker
                    .stats
                    .as_ref()
                    .and_then(|s| s.volume_usd)
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
        })
    }

    async fn fetch_option_chain(&self, underlying: &str) -> CcxtResult<OptionChain> {
        let currency = underlying.to_uppercase();
        let params = json!({
            "currency": currency,
            "kind": "option",
            "expired": false
        });

        let instruments: Vec<DeribitInstrument> = self
            .public_request("public/get_instruments", Some(params))
            .await?;

        let mut chain = OptionChain::new();

        for inst in instruments {
            let symbol = self.parse_symbol(&inst.instrument_name);

            // Fetch ticker for each option to get current pricing
            let ticker_params = json!({ "instrument_name": inst.instrument_name });
            if let Ok(ticker) = self
                .public_request::<DeribitTicker>("public/ticker", Some(ticker_params))
                .await
            {
                let contract = OptionContract {
                    info: serde_json::to_value(&inst).unwrap_or_default(),
                    currency: currency.clone(),
                    symbol: symbol.clone(),
                    timestamp: Some(ticker.timestamp),
                    datetime: chrono::DateTime::from_timestamp_millis(ticker.timestamp)
                        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
                    implied_volatility: Decimal::ZERO,
                    open_interest: Decimal::try_from(ticker.open_interest.unwrap_or(0.0))
                        .unwrap_or_default(),
                    bid_price: Decimal::try_from(ticker.best_bid_price.unwrap_or(0.0))
                        .unwrap_or_default(),
                    ask_price: Decimal::try_from(ticker.best_ask_price.unwrap_or(0.0))
                        .unwrap_or_default(),
                    mid_price: Decimal::try_from(
                        (ticker.best_bid_price.unwrap_or(0.0)
                            + ticker.best_ask_price.unwrap_or(0.0))
                            / 2.0,
                    )
                    .unwrap_or_default(),
                    mark_price: Decimal::try_from(ticker.mark_price.unwrap_or(0.0))
                        .unwrap_or_default(),
                    last_price: Decimal::try_from(ticker.last_price.unwrap_or(0.0))
                        .unwrap_or_default(),
                    underlying_price: Decimal::try_from(ticker.underlying_price.unwrap_or(0.0))
                        .unwrap_or_default(),
                    change: Decimal::try_from(
                        ticker
                            .stats
                            .as_ref()
                            .and_then(|s| s.price_change)
                            .unwrap_or(0.0),
                    )
                    .unwrap_or_default(),
                    percentage: Decimal::try_from(
                        ticker
                            .stats
                            .as_ref()
                            .and_then(|s| s.price_change)
                            .unwrap_or(0.0),
                    )
                    .unwrap_or_default(),
                    base_volume: Decimal::try_from(
                        ticker.stats.as_ref().and_then(|s| s.volume).unwrap_or(0.0),
                    )
                    .unwrap_or_default(),
                    quote_volume: Decimal::try_from(
                        ticker
                            .stats
                            .as_ref()
                            .and_then(|s| s.volume_usd)
                            .unwrap_or(0.0),
                    )
                    .unwrap_or_default(),
                };
                chain.insert(symbol, contract);
            }
        }

        Ok(chain)
    }

    async fn fetch_greeks(&self, symbol: &str) -> CcxtResult<Greeks> {
        let instrument_name = self.market_id(symbol).unwrap_or_else(|| symbol.to_string());

        let params = json!({ "instrument_name": instrument_name });
        let ticker: DeribitTicker = self.public_request("public/ticker", Some(params)).await?;

        let greeks = ticker.greeks.as_ref();

        Ok(Greeks {
            symbol: symbol.to_string(),
            timestamp: Some(ticker.timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(ticker.timestamp)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
            delta: Decimal::try_from(greeks.and_then(|g| g.delta).unwrap_or(0.0))
                .unwrap_or_default(),
            gamma: Decimal::try_from(greeks.and_then(|g| g.gamma).unwrap_or(0.0))
                .unwrap_or_default(),
            theta: Decimal::try_from(greeks.and_then(|g| g.theta).unwrap_or(0.0))
                .unwrap_or_default(),
            vega: Decimal::try_from(greeks.and_then(|g| g.vega).unwrap_or(0.0)).unwrap_or_default(),
            rho: Decimal::try_from(greeks.and_then(|g| g.rho).unwrap_or(0.0)).unwrap_or_default(),
            vanna: None,
            volga: None,
            charm: None,
            bid_size: Decimal::try_from(ticker.best_bid_amount.unwrap_or(0.0)).unwrap_or_default(),
            ask_size: Decimal::try_from(ticker.best_ask_amount.unwrap_or(0.0)).unwrap_or_default(),
            bid_implied_volatility: Decimal::ZERO,
            ask_implied_volatility: Decimal::ZERO,
            mark_implied_volatility: Decimal::ZERO,
            bid_price: Decimal::try_from(ticker.best_bid_price.unwrap_or(0.0)).unwrap_or_default(),
            ask_price: Decimal::try_from(ticker.best_ask_price.unwrap_or(0.0)).unwrap_or_default(),
            mark_price: Decimal::try_from(ticker.mark_price.unwrap_or(0.0)).unwrap_or_default(),
            last_price: Decimal::try_from(ticker.last_price.unwrap_or(0.0)).unwrap_or_default(),
            underlying_price: Decimal::try_from(ticker.underlying_price.unwrap_or(0.0))
                .unwrap_or_default(),
            info: serde_json::to_value(&ticker).unwrap_or_default(),
        })
    }

    async fn fetch_underlying_assets(&self) -> CcxtResult<Vec<String>> {
        // Deribit supports BTC and ETH as underlying assets for options
        Ok(vec!["BTC".to_string(), "ETH".to_string()])
    }

    fn sign(
        &self,
        _path: &str,
        _api: &str,
        _method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        // Deribit uses JSON-RPC with bearer token, so this returns a placeholder
        SignedRequest {
            url: String::new(),
            method: String::new(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deribit_creation() {
        let config = ExchangeConfig::new();
        let exchange = Deribit::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_order_status_parsing() {
        assert!(matches!(
            Deribit::parse_order_status("open"),
            OrderStatus::Open
        ));
        assert!(matches!(
            Deribit::parse_order_status("filled"),
            OrderStatus::Closed
        ));
        assert!(matches!(
            Deribit::parse_order_status("cancelled"),
            OrderStatus::Canceled
        ));
    }

    #[test]
    fn test_order_side_parsing() {
        assert!(matches!(Deribit::parse_order_side("buy"), OrderSide::Buy));
        assert!(matches!(Deribit::parse_order_side("sell"), OrderSide::Sell));
    }

    #[test]
    fn test_symbol_parsing() {
        let config = ExchangeConfig::new();
        let exchange = Deribit::new(config).unwrap();

        assert_eq!(exchange.parse_symbol("BTC-PERPETUAL"), "BTC/USD:BTC");
        assert_eq!(exchange.parse_symbol("ETH-PERPETUAL"), "ETH/USD:ETH");
    }
}
