//! BitMEX Exchange Implementation
//!
//! BitMEX 거래소 API 구현
//! Professional derivatives exchange with perpetual and futures contracts

#![allow(dead_code)]
#![allow(clippy::manual_strip)]
#![allow(clippy::if_same_then_else)]

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, TimeInForce, OHLCV,
    Position, PositionSide, MarginMode, FundingRate, FundingRateHistory, Trade,
};

const BASE_URL: &str = "https://www.bitmex.com";
const TESTNET_URL: &str = "https://testnet.bitmex.com";
const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

type HmacSha256 = Hmac<Sha256>;

/// BitMEX exchange structure
pub struct Bitmex {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    sandbox: bool,
}

// Response wrappers
#[derive(Debug, Deserialize, Serialize)]
struct BitmexInstrument {
    symbol: String,
    #[serde(rename = "rootSymbol")]
    root_symbol: Option<String>,
    state: Option<String>,
    #[serde(rename = "quoteCurrency")]
    quote_currency: Option<String>,
    #[serde(rename = "underlying")]
    underlying: Option<String>,
    #[serde(rename = "maxOrderQty")]
    max_order_qty: Option<i64>,
    #[serde(rename = "maxPrice")]
    max_price: Option<f64>,
    #[serde(rename = "lotSize")]
    lot_size: Option<i64>,
    #[serde(rename = "tickSize")]
    tick_size: Option<f64>,
    multiplier: Option<i64>,
    #[serde(rename = "settlCurrency")]
    settle_currency: Option<String>,
    #[serde(rename = "isQuanto")]
    is_quanto: Option<bool>,
    #[serde(rename = "isInverse")]
    is_inverse: Option<bool>,
    #[serde(rename = "initMargin")]
    init_margin: Option<f64>,
    #[serde(rename = "maintMargin")]
    maint_margin: Option<f64>,
    #[serde(rename = "lastPrice")]
    last_price: Option<f64>,
    #[serde(rename = "lastTickDirection")]
    last_tick_direction: Option<String>,
    #[serde(rename = "lastChangePcnt")]
    last_change_pcnt: Option<f64>,
    #[serde(rename = "bidPrice")]
    bid_price: Option<f64>,
    #[serde(rename = "midPrice")]
    mid_price: Option<f64>,
    #[serde(rename = "askPrice")]
    ask_price: Option<f64>,
    #[serde(rename = "impactBidPrice")]
    impact_bid_price: Option<f64>,
    #[serde(rename = "impactAskPrice")]
    impact_ask_price: Option<f64>,
    #[serde(rename = "openInterest")]
    open_interest: Option<i64>,
    #[serde(rename = "openValue")]
    open_value: Option<i64>,
    #[serde(rename = "fairPrice")]
    fair_price: Option<f64>,
    #[serde(rename = "markPrice")]
    mark_price: Option<f64>,
    #[serde(rename = "fundingRate")]
    funding_rate: Option<f64>,
    #[serde(rename = "fundingTimestamp")]
    funding_timestamp: Option<String>,
    #[serde(rename = "highPrice")]
    high_price: Option<f64>,
    #[serde(rename = "lowPrice")]
    low_price: Option<f64>,
    #[serde(rename = "prevClosePrice")]
    prev_close_price: Option<f64>,
    volume: Option<i64>,
    #[serde(rename = "volume24h")]
    volume_24h: Option<i64>,
    turnover: Option<i64>,
    #[serde(rename = "turnover24h")]
    turnover_24h: Option<i64>,
    #[serde(rename = "takerFee")]
    taker_fee: Option<f64>,
    #[serde(rename = "makerFee")]
    maker_fee: Option<f64>,
    timestamp: Option<String>,
    expiry: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitmexOrderBookEntry {
    symbol: String,
    id: i64,
    side: String,
    size: i64,
    price: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexTrade {
    timestamp: String,
    symbol: String,
    side: String,
    size: i64,
    price: f64,
    #[serde(rename = "tickDirection")]
    tick_direction: Option<String>,
    #[serde(rename = "trdMatchID")]
    trd_match_id: String,
    #[serde(rename = "grossValue")]
    gross_value: Option<i64>,
    #[serde(rename = "homeNotional")]
    home_notional: Option<f64>,
    #[serde(rename = "foreignNotional")]
    foreign_notional: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BitmexOhlcv {
    timestamp: String,
    symbol: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    trades: Option<i64>,
    volume: i64,
    #[serde(rename = "vwap")]
    vwap: Option<f64>,
    #[serde(rename = "lastSize")]
    last_size: Option<i64>,
    turnover: Option<i64>,
    #[serde(rename = "homeNotional")]
    home_notional: Option<f64>,
    #[serde(rename = "foreignNotional")]
    foreign_notional: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexWallet {
    account: i64,
    currency: String,
    #[serde(rename = "prevDeposited")]
    prev_deposited: Option<i64>,
    #[serde(rename = "prevWithdrawn")]
    prev_withdrawn: Option<i64>,
    #[serde(rename = "prevTransferIn")]
    prev_transfer_in: Option<i64>,
    #[serde(rename = "prevTransferOut")]
    prev_transfer_out: Option<i64>,
    #[serde(rename = "prevAmount")]
    prev_amount: Option<i64>,
    #[serde(rename = "prevTimestamp")]
    prev_timestamp: Option<String>,
    #[serde(rename = "deltaDeposited")]
    delta_deposited: Option<i64>,
    #[serde(rename = "deltaWithdrawn")]
    delta_withdrawn: Option<i64>,
    #[serde(rename = "deltaTransferIn")]
    delta_transfer_in: Option<i64>,
    #[serde(rename = "deltaTransferOut")]
    delta_transfer_out: Option<i64>,
    #[serde(rename = "deltaAmount")]
    delta_amount: Option<i64>,
    deposited: Option<i64>,
    withdrawn: Option<i64>,
    #[serde(rename = "transferIn")]
    transfer_in: Option<i64>,
    #[serde(rename = "transferOut")]
    transfer_out: Option<i64>,
    amount: i64,
    #[serde(rename = "pendingCredit")]
    pending_credit: Option<i64>,
    #[serde(rename = "pendingDebit")]
    pending_debit: Option<i64>,
    #[serde(rename = "confirmedDebit")]
    confirmed_debit: Option<i64>,
    timestamp: Option<String>,
    addr: Option<String>,
    script: Option<String>,
    #[serde(rename = "withdrawalLock")]
    withdrawal_lock: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexOrder {
    #[serde(rename = "orderID")]
    order_id: String,
    #[serde(rename = "clOrdID")]
    cl_ord_id: Option<String>,
    #[serde(rename = "clOrdLinkID")]
    cl_ord_link_id: Option<String>,
    account: Option<i64>,
    symbol: String,
    side: String,
    #[serde(rename = "simpleOrderQty")]
    simple_order_qty: Option<f64>,
    #[serde(rename = "orderQty")]
    order_qty: Option<i64>,
    price: Option<f64>,
    #[serde(rename = "displayQty")]
    display_qty: Option<i64>,
    #[serde(rename = "stopPx")]
    stop_px: Option<f64>,
    #[serde(rename = "pegOffsetValue")]
    peg_offset_value: Option<f64>,
    #[serde(rename = "pegPriceType")]
    peg_price_type: Option<String>,
    currency: Option<String>,
    #[serde(rename = "settlCurrency")]
    settl_currency: Option<String>,
    #[serde(rename = "ordType")]
    ord_type: String,
    #[serde(rename = "timeInForce")]
    time_in_force: Option<String>,
    #[serde(rename = "execInst")]
    exec_inst: Option<String>,
    #[serde(rename = "contingencyType")]
    contingency_type: Option<String>,
    #[serde(rename = "exDestination")]
    ex_destination: Option<String>,
    #[serde(rename = "ordStatus")]
    ord_status: String,
    triggered: Option<String>,
    #[serde(rename = "workingIndicator")]
    working_indicator: Option<bool>,
    #[serde(rename = "ordRejReason")]
    ord_rej_reason: Option<String>,
    #[serde(rename = "simpleLeavesQty")]
    simple_leaves_qty: Option<f64>,
    #[serde(rename = "leavesQty")]
    leaves_qty: Option<i64>,
    #[serde(rename = "simpleCumQty")]
    simple_cum_qty: Option<f64>,
    #[serde(rename = "cumQty")]
    cum_qty: Option<i64>,
    #[serde(rename = "avgPx")]
    avg_px: Option<f64>,
    #[serde(rename = "multiLegReportingType")]
    multi_leg_reporting_type: Option<String>,
    text: Option<String>,
    #[serde(rename = "transactTime")]
    transact_time: Option<String>,
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexPosition {
    account: i64,
    symbol: String,
    currency: Option<String>,
    underlying: Option<String>,
    #[serde(rename = "quoteCurrency")]
    quote_currency: Option<String>,
    commission: Option<f64>,
    #[serde(rename = "initMarginReq")]
    init_margin_req: Option<f64>,
    #[serde(rename = "maintMarginReq")]
    maint_margin_req: Option<f64>,
    #[serde(rename = "riskLimit")]
    risk_limit: Option<i64>,
    leverage: Option<f64>,
    #[serde(rename = "crossMargin")]
    cross_margin: Option<bool>,
    #[serde(rename = "deleveragePercentile")]
    deleverage_percentile: Option<f64>,
    #[serde(rename = "rebalancedPnl")]
    rebalanced_pnl: Option<i64>,
    #[serde(rename = "prevRealisedPnl")]
    prev_realised_pnl: Option<i64>,
    #[serde(rename = "prevUnrealisedPnl")]
    prev_unrealised_pnl: Option<i64>,
    #[serde(rename = "prevClosePrice")]
    prev_close_price: Option<f64>,
    #[serde(rename = "openingTimestamp")]
    opening_timestamp: Option<String>,
    #[serde(rename = "openingQty")]
    opening_qty: Option<i64>,
    #[serde(rename = "openingCost")]
    opening_cost: Option<i64>,
    #[serde(rename = "openingComm")]
    opening_comm: Option<i64>,
    #[serde(rename = "openOrderBuyQty")]
    open_order_buy_qty: Option<i64>,
    #[serde(rename = "openOrderBuyCost")]
    open_order_buy_cost: Option<i64>,
    #[serde(rename = "openOrderBuyPremium")]
    open_order_buy_premium: Option<i64>,
    #[serde(rename = "openOrderSellQty")]
    open_order_sell_qty: Option<i64>,
    #[serde(rename = "openOrderSellCost")]
    open_order_sell_cost: Option<i64>,
    #[serde(rename = "openOrderSellPremium")]
    open_order_sell_premium: Option<i64>,
    #[serde(rename = "execBuyQty")]
    exec_buy_qty: Option<i64>,
    #[serde(rename = "execBuyCost")]
    exec_buy_cost: Option<i64>,
    #[serde(rename = "execSellQty")]
    exec_sell_qty: Option<i64>,
    #[serde(rename = "execSellCost")]
    exec_sell_cost: Option<i64>,
    #[serde(rename = "execQty")]
    exec_qty: Option<i64>,
    #[serde(rename = "execCost")]
    exec_cost: Option<i64>,
    #[serde(rename = "execComm")]
    exec_comm: Option<i64>,
    #[serde(rename = "currentTimestamp")]
    current_timestamp: Option<String>,
    #[serde(rename = "currentQty")]
    current_qty: i64,
    #[serde(rename = "currentCost")]
    current_cost: Option<i64>,
    #[serde(rename = "currentComm")]
    current_comm: Option<i64>,
    #[serde(rename = "realisedCost")]
    realised_cost: Option<i64>,
    #[serde(rename = "unrealisedCost")]
    unrealised_cost: Option<i64>,
    #[serde(rename = "grossOpenCost")]
    gross_open_cost: Option<i64>,
    #[serde(rename = "grossOpenPremium")]
    gross_open_premium: Option<i64>,
    #[serde(rename = "grossExecCost")]
    gross_exec_cost: Option<i64>,
    #[serde(rename = "isOpen")]
    is_open: Option<bool>,
    #[serde(rename = "markPrice")]
    mark_price: Option<f64>,
    #[serde(rename = "markValue")]
    mark_value: Option<i64>,
    #[serde(rename = "riskValue")]
    risk_value: Option<i64>,
    #[serde(rename = "homeNotional")]
    home_notional: Option<f64>,
    #[serde(rename = "foreignNotional")]
    foreign_notional: Option<f64>,
    #[serde(rename = "posState")]
    pos_state: Option<String>,
    #[serde(rename = "posCost")]
    pos_cost: Option<i64>,
    #[serde(rename = "posCost2")]
    pos_cost2: Option<i64>,
    #[serde(rename = "posCross")]
    pos_cross: Option<i64>,
    #[serde(rename = "posInit")]
    pos_init: Option<i64>,
    #[serde(rename = "posComm")]
    pos_comm: Option<i64>,
    #[serde(rename = "posLoss")]
    pos_loss: Option<i64>,
    #[serde(rename = "posMargin")]
    pos_margin: Option<i64>,
    #[serde(rename = "posMaint")]
    pos_maint: Option<i64>,
    #[serde(rename = "posAllowance")]
    pos_allowance: Option<i64>,
    #[serde(rename = "taxableMargin")]
    taxable_margin: Option<i64>,
    #[serde(rename = "initMargin")]
    init_margin: Option<i64>,
    #[serde(rename = "maintMargin")]
    maint_margin: Option<i64>,
    #[serde(rename = "sessionMargin")]
    session_margin: Option<i64>,
    #[serde(rename = "targetExcessMargin")]
    target_excess_margin: Option<i64>,
    #[serde(rename = "varMargin")]
    var_margin: Option<i64>,
    #[serde(rename = "realisedGrossPnl")]
    realised_gross_pnl: Option<i64>,
    #[serde(rename = "realisedTax")]
    realised_tax: Option<i64>,
    #[serde(rename = "realisedPnl")]
    realised_pnl: Option<i64>,
    #[serde(rename = "unrealisedGrossPnl")]
    unrealised_gross_pnl: Option<i64>,
    #[serde(rename = "longBankrupt")]
    long_bankrupt: Option<i64>,
    #[serde(rename = "shortBankrupt")]
    short_bankrupt: Option<i64>,
    #[serde(rename = "taxBase")]
    tax_base: Option<i64>,
    #[serde(rename = "indicativeTaxRate")]
    indicative_tax_rate: Option<f64>,
    #[serde(rename = "indicativeTax")]
    indicative_tax: Option<i64>,
    #[serde(rename = "unrealisedTax")]
    unrealised_tax: Option<i64>,
    #[serde(rename = "unrealisedPnl")]
    unrealised_pnl: Option<i64>,
    #[serde(rename = "unrealisedPnlPcnt")]
    unrealised_pnl_pcnt: Option<f64>,
    #[serde(rename = "unrealisedRoePcnt")]
    unrealised_roe_pcnt: Option<f64>,
    #[serde(rename = "simpleQty")]
    simple_qty: Option<f64>,
    #[serde(rename = "simpleCost")]
    simple_cost: Option<f64>,
    #[serde(rename = "simpleValue")]
    simple_value: Option<f64>,
    #[serde(rename = "simplePnl")]
    simple_pnl: Option<f64>,
    #[serde(rename = "simplePnlPcnt")]
    simple_pnl_pcnt: Option<f64>,
    #[serde(rename = "avgCostPrice")]
    avg_cost_price: Option<f64>,
    #[serde(rename = "avgEntryPrice")]
    avg_entry_price: Option<f64>,
    #[serde(rename = "breakEvenPrice")]
    break_even_price: Option<f64>,
    #[serde(rename = "marginCallPrice")]
    margin_call_price: Option<f64>,
    #[serde(rename = "liquidationPrice")]
    liquidation_price: Option<f64>,
    #[serde(rename = "bankruptPrice")]
    bankrupt_price: Option<f64>,
    timestamp: Option<String>,
    #[serde(rename = "lastPrice")]
    last_price: Option<f64>,
    #[serde(rename = "lastValue")]
    last_value: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmexFunding {
    timestamp: String,
    symbol: String,
    #[serde(rename = "fundingInterval")]
    funding_interval: Option<String>,
    #[serde(rename = "fundingRate")]
    funding_rate: f64,
    #[serde(rename = "fundingRateDaily")]
    funding_rate_daily: Option<f64>,
}

impl Bitmex {
    /// Create a new BitMEX exchange instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let sandbox = config.is_sandbox();
        let base_url = if sandbox { TESTNET_URL } else { BASE_URL };
        let client = HttpClient::new(base_url, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: false,
            margin: true,
            swap: true,
            future: true,
            option: false,
            fetch_markets: true,
            fetch_currencies: false,
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
            ws: true,
            watch_ticker: true,
            watch_order_book: true,
            watch_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".to_string(), format!("{base_url}/api/v1"));
        api_urls.insert("private".to_string(), format!("{base_url}/api/v1"));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766319-f653c6e6-5ed4-11e7-933d-f0bc3699ae8f.jpg".to_string()),
            api: api_urls,
            www: Some("https://www.bitmex.com".to_string()),
            doc: vec!["https://www.bitmex.com/app/apiOverview".to_string()],
            fees: Some("https://www.bitmex.com/app/fees".to_string()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            sandbox,
        })
    }

    /// Generate HMAC-SHA256 signature
    fn sign(&self, message: &str) -> CcxtResult<String> {
        let secret = self.config.secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API secret required".to_string(),
            })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("HMAC error: {e}"),
            })?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Make authenticated request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
        body: Option<Value>,
    ) -> CcxtResult<T> {
        let api_key = self.config.api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;

        self.rate_limiter.throttle(1.0).await;

        let expires = chrono::Utc::now().timestamp() + 5;
        let query_string = params.as_ref()
            .map(|p| {
                p.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&")
            })
            .unwrap_or_default();

        let body_str = body.as_ref()
            .map(|b| b.to_string())
            .unwrap_or_default();

        let full_path = if query_string.is_empty() {
            format!("/api/v1{path}")
        } else {
            format!("/api/v1{path}?{query_string}")
        };

        let message = format!("{method}{full_path}{expires}{body_str}");
        let signature = self.sign(&message)?;

        let mut headers = HashMap::new();
        headers.insert("api-expires".to_string(), expires.to_string());
        headers.insert("api-key".to_string(), api_key.to_string());
        headers.insert("api-signature".to_string(), signature);
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        match method {
            "GET" => {
                self.client.get(&full_path, None, Some(headers)).await
            }
            "POST" => {
                self.client.post(&full_path, body, Some(headers)).await
            }
            "DELETE" => {
                self.client.delete(&full_path, None, Some(headers)).await
            }
            "PUT" => {
                if let Some(params) = params {
                    self.client.put(&full_path, params, Some(headers)).await
                } else {
                    self.client.put(&full_path, HashMap::new(), Some(headers)).await
                }
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Make public request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Parse ISO 8601 timestamp to milliseconds
    fn parse_timestamp(ts: &str) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(ts)
            .ok()
            .map(|dt| dt.timestamp_millis())
    }

    /// Parse order status
    fn parse_order_status(status: &str) -> OrderStatus {
        match status {
            "New" => OrderStatus::Open,
            "PartiallyFilled" => OrderStatus::Open,
            "Filled" => OrderStatus::Closed,
            "Canceled" => OrderStatus::Canceled,
            "Rejected" => OrderStatus::Rejected,
            "Expired" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order side
    fn parse_order_side(side: &str) -> OrderSide {
        match side.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order type
    fn parse_order_type(ord_type: &str) -> OrderType {
        match ord_type {
            "Limit" => OrderType::Limit,
            "Market" => OrderType::Market,
            "Stop" => OrderType::StopMarket,
            "StopLimit" => OrderType::StopLimit,
            "MarketIfTouched" => OrderType::TakeProfitMarket,
            _ => OrderType::Limit,
        }
    }

    /// Convert order to unified format
    fn parse_order(&self, order: &BitmexOrder) -> Order {
        let timestamp = order.timestamp.as_ref()
            .and_then(|ts| Self::parse_timestamp(ts));
        let datetime = timestamp.map(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        let side = Self::parse_order_side(&order.side);
        let status = Self::parse_order_status(&order.ord_status);
        let order_type = Self::parse_order_type(&order.ord_type);

        let amount = order.order_qty.map(Decimal::from).unwrap_or(Decimal::ZERO);
        let filled = order.cum_qty.map(Decimal::from).unwrap_or(Decimal::ZERO);
        let remaining = order.leaves_qty.map(Decimal::from).unwrap_or(Decimal::ZERO);
        let price = order.price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let average = order.avg_px.map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let stop_price = order.stop_px.map(|p| Decimal::from_f64_retain(p).unwrap_or_default());

        let cost = if let (Some(avg), Some(qty)) = (average, order.cum_qty) {
            Some(avg * Decimal::from(qty))
        } else {
            None
        };

        let time_in_force = order.time_in_force.as_ref().map(|tif| {
            match tif.as_str() {
                "GTC" => TimeInForce::GTC,
                "IOC" => TimeInForce::IOC,
                "FOK" => TimeInForce::FOK,
                "PO" => TimeInForce::PO,
                _ => TimeInForce::GTC,
            }
        });

        Order {
            id: order.order_id.clone(),
            client_order_id: order.cl_ord_id.clone(),
            timestamp,
            datetime,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: order.symbol.clone(),
            order_type,
            time_in_force,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            reduce_only: None,
            post_only: order.exec_inst.as_ref().map(|e| e.contains("ParticipateDoNotInitiate")),
            stop_price,
            trigger_price: stop_price,
            take_profit_price: None,
            stop_loss_price: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(order).unwrap_or(Value::Null),
        }
    }

    /// Convert position to unified format
    fn parse_position(&self, pos: &BitmexPosition) -> Position {
        let timestamp = pos.timestamp.as_ref()
            .and_then(|ts| Self::parse_timestamp(ts));
        let datetime = timestamp.map(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        let side = if pos.current_qty > 0 {
            Some(PositionSide::Long)
        } else if pos.current_qty < 0 {
            Some(PositionSide::Short)
        } else {
            None
        };

        let contracts = Some(Decimal::from(pos.current_qty.abs()));
        let entry_price = pos.avg_entry_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let mark_price = pos.mark_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let liquidation_price = pos.liquidation_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default());
        let leverage = pos.leverage.map(|l| Decimal::from_f64_retain(l).unwrap_or_default());
        let unrealized_pnl = pos.unrealised_pnl.map(|p| {
            // Convert from satoshis to BTC
            Decimal::from(p) / Decimal::from(100_000_000)
        });
        let realized_pnl = pos.realised_pnl.map(|p| {
            Decimal::from(p) / Decimal::from(100_000_000)
        });
        let margin_mode = if pos.cross_margin.unwrap_or(false) {
            Some(MarginMode::Cross)
        } else {
            Some(MarginMode::Isolated)
        };

        let notional = pos.foreign_notional.map(|n| Decimal::from_f64_retain(n).unwrap_or_default());
        let collateral = pos.pos_margin.map(|m| {
            Decimal::from(m) / Decimal::from(100_000_000)
        });

        Position {
            symbol: pos.symbol.clone(),
            id: None,
            info: serde_json::to_value(pos).unwrap_or(Value::Null),
            timestamp,
            datetime,
            contracts,
            contract_size: Some(Decimal::ONE),
            side,
            notional,
            leverage,
            unrealized_pnl,
            realized_pnl,
            collateral,
            entry_price,
            mark_price,
            liquidation_price,
            margin_mode,
            hedged: Some(false),
            maintenance_margin: pos.maint_margin.map(|m| Decimal::from(m) / Decimal::from(100_000_000)),
            maintenance_margin_percentage: pos.maint_margin_req.map(|r| Decimal::from_f64_retain(r).unwrap_or_default()),
            initial_margin: pos.init_margin.map(|m| Decimal::from(m) / Decimal::from(100_000_000)),
            initial_margin_percentage: pos.init_margin_req.map(|r| Decimal::from_f64_retain(r).unwrap_or_default()),
            margin_ratio: None,
            last_update_timestamp: None,
            last_price: pos.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            stop_loss_price: None,
            take_profit_price: None,
            percentage: pos.unrealised_pnl_pcnt.map(|p| Decimal::from_f64_retain(p * 100.0).unwrap_or_default()),
        }
    }
}

#[async_trait]
impl Exchange for Bitmex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitmex
    }

    fn name(&self) -> &'static str {
        "BitMEX"
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let instruments: Vec<BitmexInstrument> = self.public_get("/instrument/active", None).await?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for inst in instruments {
            let base = inst.root_symbol.clone().unwrap_or_else(|| {
                if inst.symbol.starts_with("XBT") {
                    "BTC".to_string()
                } else {
                    inst.symbol.chars().take(3).collect()
                }
            });
            let quote = inst.quote_currency.clone().unwrap_or_else(|| "USD".to_string());
            let settle = inst.settle_currency.clone().unwrap_or_else(|| "XBt".to_string());

            let is_inverse = inst.is_inverse.unwrap_or(false);
            let is_swap = inst.expiry.is_none();

            let symbol = if is_swap {
                format!("{}/{}:{}", base, quote, if settle == "XBt" { "BTC" } else { &settle })
            } else {
                format!("{}/{}:{}", base, quote, if settle == "XBt" { "BTC" } else { &settle })
            };

            let tick_size = inst.tick_size.unwrap_or(0.5);
            let lot_size = inst.lot_size.unwrap_or(1) as f64;

            let precision = MarketPrecision {
                price: Some((1.0 / tick_size).log10().ceil() as i32),
                amount: Some(0),
                cost: None,
                base: None,
                quote: None,
            };

            let limits = MarketLimits {
                amount: MinMax {
                    min: Some(Decimal::from_f64_retain(lot_size).unwrap_or(Decimal::ONE)),
                    max: inst.max_order_qty.map(Decimal::from),
                },
                price: MinMax {
                    min: Some(Decimal::from_f64_retain(tick_size).unwrap_or(Decimal::ONE)),
                    max: inst.max_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                },
                cost: MinMax { min: None, max: None },
                leverage: MinMax {
                    min: Some(Decimal::ONE),
                    max: Some(Decimal::from(100)),
                },
            };

            let market = Market {
                id: inst.symbol.clone(),
                lowercase_id: Some(inst.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                settle_id: Some(if settle == "XBt" { "BTC".to_string() } else { settle.clone() }),
                active: inst.state.as_ref().map(|s| s == "Open").unwrap_or(true),
                market_type: if is_swap { MarketType::Swap } else { MarketType::Future },
                spot: false,
                margin: true,
                swap: is_swap,
                future: !is_swap,
                option: false,
                index: false,
                contract: true,
                settle: Some(if settle == "XBt" { "BTC".to_string() } else { settle.clone() }),
                contract_size: Some(Decimal::ONE),
                linear: Some(!is_inverse),
                inverse: Some(is_inverse),
                sub_type: None,
                expiry: inst.expiry.as_ref().and_then(|e| Self::parse_timestamp(e)),
                expiry_datetime: inst.expiry.clone(),
                strike: None,
                option_type: None,
                taker: inst.taker_fee.map(|f| Decimal::from_f64_retain(f).unwrap_or_default()),
                maker: inst.maker_fee.map(|f| Decimal::from_f64_retain(f).unwrap_or_default()),
                percentage: true,
                tier_based: false,
                precision,
                limits,
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&inst).unwrap_or(Value::Null),
            };

            markets_by_id.insert(inst.symbol.clone(), symbol.clone());
            markets.insert(symbol, market);
        }

        *self.markets.write().unwrap() = markets.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(true).await?;
        Ok(markets.values().cloned().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let instruments: Vec<BitmexInstrument> = self.public_get("/instrument", Some(params)).await?;
        let inst = instruments.first()
            .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?;

        let timestamp = inst.timestamp.as_ref().and_then(|ts| Self::parse_timestamp(ts));
        let datetime = timestamp.map(|t| {
            chrono::DateTime::from_timestamp_millis(t)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime,
            high: inst.high_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            low: inst.low_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid: inst.bid_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            bid_volume: None,
            ask: inst.ask_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            ask_volume: None,
            vwap: None,
            open: inst.prev_close_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            close: inst.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            last: inst.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            previous_close: inst.prev_close_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            change: inst.last_price.zip(inst.prev_close_price)
                .map(|(last, prev)| Decimal::from_f64_retain(last - prev).unwrap_or_default()),
            percentage: inst.last_change_pcnt.map(|p| Decimal::from_f64_retain(p * 100.0).unwrap_or_default()),
            average: inst.mid_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            base_volume: inst.volume_24h.map(Decimal::from),
            quote_volume: inst.turnover_24h.map(|t| {
                // Convert from satoshis to BTC
                Decimal::from(t) / Decimal::from(100_000_000)
            }),
            index_price: None,
            mark_price: inst.mark_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
            info: serde_json::to_value(inst).unwrap_or(Value::Null),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let instruments: Vec<BitmexInstrument> = self.public_get("/instrument/active", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut result = HashMap::new();

        for inst in instruments {
            if let Some(symbol) = markets_by_id.get(&inst.symbol) {
                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let timestamp = inst.timestamp.as_ref().and_then(|ts| Self::parse_timestamp(ts));
                let datetime = timestamp.map(|t| {
                    chrono::DateTime::from_timestamp_millis(t)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                });

                let ticker = Ticker {
                    symbol: symbol.clone(),
                    timestamp,
                    datetime,
                    high: inst.high_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    low: inst.low_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    bid: inst.bid_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    bid_volume: None,
                    ask: inst.ask_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    ask_volume: None,
                    vwap: None,
                    open: inst.prev_close_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    close: inst.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    last: inst.last_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    previous_close: inst.prev_close_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    change: None,
                    percentage: inst.last_change_pcnt.map(|p| Decimal::from_f64_retain(p * 100.0).unwrap_or_default()),
                    average: None,
                    base_volume: inst.volume_24h.map(Decimal::from),
                    quote_volume: inst.turnover_24h.map(|t| Decimal::from(t) / Decimal::from(100_000_000)),
                    index_price: None,
                    mark_price: inst.mark_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                    info: serde_json::to_value(&inst).unwrap_or(Value::Null),
                };

                result.insert(symbol.clone(), ticker);
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("depth".to_string(), l.to_string());
        }

        let entries: Vec<BitmexOrderBookEntry> = self.public_get("/orderBook/L2", Some(params)).await?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for entry in entries {
            let price = Decimal::from_f64_retain(entry.price).unwrap_or_default();
            let amount = Decimal::from(entry.size);

            let book_entry = OrderBookEntry { price, amount };

            match entry.side.as_str() {
                "Buy" => bids.push(book_entry),
                "Sell" => asks.push(book_entry),
                _ => {}
            }
        }

        // Sort bids descending, asks ascending
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));

        let timestamp = chrono::Utc::now().timestamp_millis();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(chrono::Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
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
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("reverse".to_string(), "true".to_string());
        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let trades: Vec<BitmexTrade> = self.public_get("/trade", Some(params)).await?;

        let result: Vec<Trade> = trades.iter()
            .map(|t| {
                let timestamp = Self::parse_timestamp(&t.timestamp);
                let datetime = timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                });

                let side = match t.side.to_lowercase().as_str() {
                    "buy" => "buy",
                    "sell" => "sell",
                    _ => "buy",
                };

                let price = Decimal::from_f64_retain(t.price).unwrap_or_default();
                let amount = Decimal::from(t.size);

                Trade {
                    id: t.trd_match_id.clone(),
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

        Ok(result)
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
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let bin_size = self.timeframes.get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1h".to_string());

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("binSize".to_string(), bin_size);
        params.insert("partial".to_string(), "true".to_string());
        params.insert("reverse".to_string(), "true".to_string());

        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        if let Some(s) = since {
            let start_time = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("startTime".to_string(), start_time);
        }

        let candles: Vec<BitmexOhlcv> = self.public_get("/trade/bucketed", Some(params)).await?;

        let mut result: Vec<OHLCV> = candles.iter()
            .filter_map(|c| {
                let timestamp = Self::parse_timestamp(&c.timestamp)?;
                Some(OHLCV {
                    timestamp,
                    open: Decimal::from_f64_retain(c.open).unwrap_or_default(),
                    high: Decimal::from_f64_retain(c.high).unwrap_or_default(),
                    low: Decimal::from_f64_retain(c.low).unwrap_or_default(),
                    close: Decimal::from_f64_retain(c.close).unwrap_or_default(),
                    volume: Decimal::from(c.volume),
                })
            })
            .collect();

        // Reverse to get chronological order
        result.reverse();

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let wallet: BitmexWallet = self.private_request("GET", "/user/wallet", None, None).await?;

        let mut result = Balances::new();

        // Convert from satoshis to BTC
        let amount_dec = Decimal::from(wallet.amount) / Decimal::from(100_000_000);
        let pending_credit = wallet.pending_credit.unwrap_or(0);
        let pending_debit = wallet.pending_debit.unwrap_or(0);
        let available = Decimal::from(wallet.amount - pending_debit) / Decimal::from(100_000_000);
        let pending = Decimal::from(pending_credit + pending_debit) / Decimal::from(100_000_000);

        let currency = if wallet.currency == "XBt" { "BTC" } else { &wallet.currency };

        result.currencies.insert(currency.to_string(), Balance {
            free: Some(available),
            used: Some(pending),
            total: Some(amount_dec),
            debt: None,
        });

        result.info = serde_json::to_value(&wallet).unwrap_or(Value::Null);

        Ok(result)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("filter".to_string(), r#"{"open":true}"#.to_string());

        if let Some(sym) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets.get(sym)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol { symbol: sym.to_string() })?
            };
            params.insert("symbol".to_string(), market_id);
        }

        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        let orders: Vec<BitmexOrder> = self.private_request("GET", "/order", Some(params), None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let result: Vec<Order> = orders.iter()
            .map(|o| {
                let mut order = self.parse_order(o);
                if let Some(symbol) = markets_by_id.get(&o.symbol) {
                    order.symbol = symbol.clone();
                }
                order
            })
            .collect();

        Ok(result)
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("filter".to_string(), format!(r#"{{"orderID":"{id}"}}"#));

        let orders: Vec<BitmexOrder> = self.private_request("GET", "/order", Some(params), None).await?;

        let order = orders.first()
            .ok_or_else(|| CcxtError::OrderNotFound { order_id: id.to_string() })?;

        let mut result = self.parse_order(order);
        result.symbol = symbol.to_string();

        Ok(result)
    }

    async fn fetch_positions(&self, symbols: Option<&[&str]>) -> CcxtResult<Vec<Position>> {
        self.load_markets(false).await?;

        let positions: Vec<BitmexPosition> = self.private_request("GET", "/position", None, None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let result: Vec<Position> = positions.iter()
            .filter(|p| p.current_qty != 0)
            .filter_map(|p| {
                let symbol = markets_by_id.get(&p.symbol)?;

                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        return None;
                    }
                }

                let mut pos = self.parse_position(p);
                pos.symbol = symbol.clone();
                Some(pos)
            })
            .collect();

        Ok(result)
    }

    async fn fetch_funding_rates(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, FundingRate>> {
        self.load_markets(false).await?;

        let instruments: Vec<BitmexInstrument> = self.public_get("/instrument/active", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut result = HashMap::new();

        for inst in instruments {
            if let Some(symbol) = markets_by_id.get(&inst.symbol) {
                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                if let Some(rate) = inst.funding_rate {
                    let timestamp = inst.timestamp.as_ref().and_then(|ts| Self::parse_timestamp(ts));
                    let datetime = timestamp.map(|t| {
                        chrono::DateTime::from_timestamp_millis(t)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    });

                    let next_funding_time = inst.funding_timestamp.as_ref()
                        .and_then(|ts| Self::parse_timestamp(ts));

                    result.insert(symbol.clone(), FundingRate {
                        info: serde_json::to_value(&inst).unwrap_or(Value::Null),
                        symbol: symbol.clone(),
                        mark_price: inst.mark_price.map(|p| Decimal::from_f64_retain(p).unwrap_or_default()),
                        index_price: None,
                        interest_rate: None,
                        estimated_settle_price: None,
                        timestamp,
                        datetime,
                        funding_rate: Some(Decimal::from_f64_retain(rate).unwrap_or_default()),
                        funding_timestamp: next_funding_time,
                        funding_datetime: inst.funding_timestamp.clone(),
                        next_funding_rate: None,
                        next_funding_timestamp: None,
                        next_funding_datetime: None,
                        previous_funding_rate: None,
                        previous_funding_timestamp: None,
                        previous_funding_datetime: None,
                        interval: Some("8h".to_string()),
                    });
                }
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
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("reverse".to_string(), "true".to_string());

        if let Some(l) = limit {
            params.insert("count".to_string(), l.to_string());
        }

        if let Some(s) = since {
            let start_time = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("startTime".to_string(), start_time);
        }

        let funding: Vec<BitmexFunding> = self.public_get("/funding", Some(params)).await?;

        let result: Vec<FundingRateHistory> = funding.iter()
            .filter_map(|f| {
                let timestamp = Self::parse_timestamp(&f.timestamp)?;
                let datetime = chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339());

                Some(FundingRateHistory {
                    info: serde_json::to_value(f).unwrap_or(Value::Null),
                    symbol: symbol.to_string(),
                    funding_rate: Decimal::from_f64_retain(f.funding_rate).unwrap_or_default(),
                    timestamp: Some(timestamp),
                    datetime,
                })
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
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol { symbol: symbol.to_string() })?
        };

        let side_str = match side {
            OrderSide::Buy => "Buy",
            OrderSide::Sell => "Sell",
        };

        let ord_type = match order_type {
            OrderType::Market => "Market",
            OrderType::Limit => "Limit",
            OrderType::StopMarket => "Stop",
            OrderType::StopLimit => "StopLimit",
            _ => "Limit",
        };

        let mut body = json!({
            "symbol": market_id,
            "side": side_str,
            "ordType": ord_type,
            "orderQty": amount.to_string().parse::<i64>().unwrap_or(0),
        });

        if let Some(p) = price {
            body["price"] = json!(p.to_string().parse::<f64>().unwrap_or(0.0));
        }

        let order: BitmexOrder = self.private_request("POST", "/order", None, Some(body)).await?;

        let mut result = self.parse_order(&order);
        result.symbol = symbol.to_string();

        Ok(result)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let body = json!({
            "orderID": id
        });

        let orders: Vec<BitmexOrder> = self.private_request("DELETE", "/order", None, Some(body)).await?;

        let order = orders.first()
            .ok_or_else(|| CcxtError::OrderNotFound { order_id: id.to_string() })?;

        Ok(self.parse_order(order))
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

    fn sign(
        &self,
        _path: &str,
        _api: &str,
        _method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        // BitMEX uses private_request directly, so this returns a placeholder
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
    fn test_bitmex_creation() {
        let config = ExchangeConfig::new();
        let exchange = Bitmex::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_order_status_parsing() {
        assert!(matches!(Bitmex::parse_order_status("New"), OrderStatus::Open));
        assert!(matches!(Bitmex::parse_order_status("Filled"), OrderStatus::Closed));
        assert!(matches!(Bitmex::parse_order_status("Canceled"), OrderStatus::Canceled));
    }

    #[test]
    fn test_order_side_parsing() {
        assert!(matches!(Bitmex::parse_order_side("Buy"), OrderSide::Buy));
        assert!(matches!(Bitmex::parse_order_side("Sell"), OrderSide::Sell));
    }
}
