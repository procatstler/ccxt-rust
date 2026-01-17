//! OX.FUN Exchange Implementation
//!
//! OX.FUN is a cryptocurrency derivatives exchange
//! API Documentation: <https://docs.ox.fun/>

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// OX.FUN Exchange
pub struct Oxfun {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// API Response structures
#[derive(Debug, Deserialize, Serialize)]
struct OxfunResponse<T> {
    success: bool,
    data: Option<T>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunMarket {
    #[serde(rename = "marketCode")]
    market_code: String,
    name: Option<String>,
    #[serde(rename = "referencePair")]
    reference_pair: Option<String>,
    base: Option<String>,
    counter: Option<String>,
    #[serde(rename = "type")]
    market_type: Option<String>,
    #[serde(rename = "tickSize")]
    tick_size: Option<String>,
    #[serde(rename = "qtyIncrement")]
    qty_increment: Option<String>,
    #[serde(rename = "minSize")]
    min_size: Option<String>,
    #[serde(rename = "maxSize")]
    max_size: Option<String>,
    #[serde(rename = "marginCurrency")]
    margin_currency: Option<String>,
    #[serde(rename = "contractValCurrency")]
    contract_val_currency: Option<String>,
    #[serde(rename = "listingDate")]
    listing_date: Option<i64>,
    status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunTicker {
    #[serde(rename = "marketCode")]
    market_code: String,
    #[serde(rename = "lastTradedPrice")]
    last_price: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(rename = "high24h")]
    high_24h: Option<String>,
    #[serde(rename = "low24h")]
    low_24h: Option<String>,
    #[serde(rename = "volume24h")]
    volume_24h: Option<String>,
    #[serde(rename = "currencyVolume24h")]
    base_volume_24h: Option<String>,
    #[serde(rename = "open24h")]
    open_24h: Option<String>,
    #[serde(rename = "openInterest")]
    open_interest: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunOrderBook {
    #[serde(rename = "marketCode")]
    market_code: Option<String>,
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
    #[serde(rename = "lastUpdatedAt")]
    last_updated_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunTrade {
    #[serde(rename = "matchId")]
    id: Option<String>,
    #[serde(rename = "matchedAt")]
    timestamp: Option<i64>,
    #[serde(rename = "matchPrice")]
    price: Option<String>,
    #[serde(rename = "matchQuantity")]
    amount: Option<String>,
    side: Option<String>,
    #[serde(rename = "matchType")]
    match_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunCandle {
    #[serde(rename = "openedAt")]
    timestamp: Option<i64>,
    open: Option<String>,
    high: Option<String>,
    low: Option<String>,
    close: Option<String>,
    volume: Option<String>,
    #[serde(rename = "currencyVolume")]
    base_volume: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunBalance {
    #[serde(rename = "instrumentId")]
    currency: Option<String>,
    total: Option<String>,
    available: Option<String>,
    reserved: Option<String>,
    #[serde(rename = "quantityLastUpdatedAt")]
    updated_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunOrder {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(rename = "marketCode")]
    market_code: Option<String>,
    side: Option<String>,
    #[serde(rename = "orderType")]
    order_type: Option<String>,
    quantity: Option<String>,
    price: Option<String>,
    #[serde(rename = "stopPrice")]
    stop_price: Option<String>,
    #[serde(rename = "filledQuantity")]
    filled: Option<String>,
    #[serde(rename = "remainingQuantity")]
    remaining: Option<String>,
    status: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<i64>,
    #[serde(rename = "timeInForce")]
    time_in_force: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OxfunMyTrade {
    #[serde(rename = "matchId")]
    trade_id: Option<String>,
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    #[serde(rename = "matchedAt")]
    timestamp: Option<i64>,
    #[serde(rename = "marketCode")]
    market_code: Option<String>,
    side: Option<String>,
    #[serde(rename = "matchPrice")]
    price: Option<String>,
    #[serde(rename = "matchQuantity")]
    amount: Option<String>,
    fees: Option<String>,
    #[serde(rename = "feeInstrumentId")]
    fee_currency: Option<String>,
    #[serde(rename = "matchType")]
    match_type: Option<String>,
}

impl Oxfun {
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".to_string());
        timeframes.insert(Timeframe::Minute5, "300".to_string());
        timeframes.insert(Timeframe::Minute15, "900".to_string());
        timeframes.insert(Timeframe::Minute30, "1800".to_string());
        timeframes.insert(Timeframe::Hour1, "3600".to_string());
        timeframes.insert(Timeframe::Hour2, "7200".to_string());
        timeframes.insert(Timeframe::Hour4, "14400".to_string());
        timeframes.insert(Timeframe::Hour6, "21600".to_string());
        timeframes.insert(Timeframe::Hour12, "43200".to_string());
        timeframes.insert(Timeframe::Day1, "86400".to_string());

        let features = ExchangeFeatures {
            spot: true,
            swap: true,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposit_address: true,
            withdraw: true,
            fetch_positions: true,
            fetch_funding_rate: true,
            fetch_funding_rate_history: true,
            ..Default::default()
        };

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/oxfun-logo.png".to_string()),
            api: HashMap::from([
                ("public".to_string(), "https://api.ox.fun".to_string()),
                ("private".to_string(), "https://api.ox.fun".to_string()),
            ]),
            www: Some("https://ox.fun".to_string()),
            doc: vec!["https://docs.ox.fun/".to_string()],
            fees: Some("https://ox.fun/fees".to_string()),
        };

        let client = HttpClient::new("https://api.ox.fun", &config)?;
        let rate_limiter = RateLimiter::new(100); // 100ms rate limit

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
        })
    }

    fn generate_signature(
        &self,
        timestamp: &str,
        nonce: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> String {
        if let Some(secret) = self.config.secret() {
            // Message format: timestamp\nnonce\nmethod\nhost\npath\nbody
            let host = "api.ox.fun";
            let msg = format!("{timestamp}\n{nonce}\n{method}\n{host}\n{path}\n{body}");

            if let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) {
                mac.update(msg.as_bytes());
                let result = mac.finalize();
                return BASE64.encode(result.into_bytes());
            }
        }
        String::new()
    }

    fn safe_symbol(&self, market_id: &str) -> String {
        if let Ok(markets_by_id) = self.markets_by_id.read() {
            if let Some(symbol) = markets_by_id.get(market_id) {
                return symbol.clone();
            }
        }
        // Parse market_id like "BTC-USD-SWAP-LIN" to "BTC/USD"
        let parts: Vec<&str> = market_id.split('-').collect();
        if parts.len() >= 2 {
            format!("{}/{}", parts[0], parts[1])
        } else {
            market_id.to_string()
        }
    }

    fn safe_market_id(&self, symbol: &str) -> String {
        if let Ok(markets) = self.markets.read() {
            if let Some(market) = markets.get(symbol) {
                return market.id.clone();
            }
        }
        symbol.replace('/', "-")
    }

    fn decimal_places(value: &Decimal) -> i32 {
        let s = value.to_string();
        if let Some(pos) = s.find('.') {
            (s.len() - pos - 1) as i32
        } else {
            0
        }
    }

    fn parse_market(&self, market_data: &OxfunMarket) -> Market {
        let market_code = &market_data.market_code;

        // Parse base and quote from market_code (e.g., "BTC-USD-SWAP-LIN" or "BTC-USD")
        let parts: Vec<&str> = market_code.split('-').collect();
        let base = market_data
            .base
            .clone()
            .unwrap_or_else(|| parts.first().map(|s| s.to_string()).unwrap_or_default());
        let quote = market_data
            .counter
            .clone()
            .unwrap_or_else(|| parts.get(1).map(|s| s.to_string()).unwrap_or_default());

        let market_type_str = market_data.market_type.as_deref().unwrap_or("");
        let is_swap = market_code.contains("SWAP") || market_type_str == "FUTURE";
        let is_linear = market_code.contains("LIN") || market_type_str.contains("LINEAR");

        let symbol = format!("{base}/{quote}");

        let tick_size = market_data
            .tick_size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::new(1, 8));

        let step_size = market_data
            .qty_increment
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::new(1, 8));

        let price_precision = Self::decimal_places(&tick_size);
        let amount_precision = Self::decimal_places(&step_size);

        let min_amount = market_data
            .min_size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let max_amount = market_data
            .max_size
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let active = market_data.status.as_deref() != Some("HALTED");

        Market {
            id: market_code.clone(),
            lowercase_id: Some(market_code.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base.clone(),
            quote_id: quote.clone(),
            settle: market_data.margin_currency.clone(),
            settle_id: market_data.margin_currency.clone(),
            active,
            market_type: if is_swap {
                MarketType::Swap
            } else {
                MarketType::Spot
            },
            spot: !is_swap,
            margin: false,
            swap: is_swap,
            future: false,
            option: false,
            index: false,
            contract: is_swap,
            linear: Some(is_linear),
            inverse: Some(!is_linear && is_swap),
            sub_type: if is_swap { Some("linear".into()) } else { None },
            contract_size: if is_swap { Some(Decimal::ONE) } else { None },
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            taker: None,
            maker: None,
            percentage: true,
            tier_based: false,
            precision: MarketPrecision {
                price: Some(price_precision),
                amount: Some(amount_precision),
                cost: None,
                base: Some(amount_precision),
                quote: Some(price_precision),
            },
            limits: MarketLimits {
                amount: crate::types::MinMax {
                    min: min_amount,
                    max: max_amount,
                },
                price: crate::types::MinMax {
                    min: Some(tick_size),
                    max: None,
                },
                cost: crate::types::MinMax {
                    min: None,
                    max: None,
                },
                leverage: crate::types::MinMax {
                    min: None,
                    max: None,
                },
            },
            margin_modes: None,
            created: market_data.listing_date,
            info: serde_json::to_value(market_data).unwrap_or_default(),
        }
    }

    fn parse_ticker(&self, ticker_data: &OxfunTicker) -> Ticker {
        let symbol = self.safe_symbol(&ticker_data.market_code);

        let last = ticker_data
            .last_price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let high = ticker_data
            .high_24h
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let low = ticker_data
            .low_24h
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let open = ticker_data
            .open_24h
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let quote_volume = ticker_data
            .volume_24h
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let base_volume = ticker_data
            .base_volume_24h
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());

        let change = match (last, open) {
            (Some(l), Some(o)) if o != Decimal::ZERO => Some(l - o),
            _ => None,
        };
        let percentage = match (change, open) {
            (Some(c), Some(o)) if o != Decimal::ZERO => Some(c / o * Decimal::from(100)),
            _ => None,
        };

        Ticker {
            symbol,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            high,
            low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker_data).unwrap_or_default(),
        }
    }

    fn parse_order(&self, order_data: &OxfunOrder) -> Order {
        let market_code = order_data.market_code.as_deref().unwrap_or("");
        let symbol = self.safe_symbol(market_code);

        let side = match order_data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match order_data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("STOP_LIMIT") => OrderType::Limit,
            Some("STOP_MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let status = match order_data.status.as_deref() {
            Some("OPEN") | Some("PARTIAL_FILL") => OrderStatus::Open,
            Some("FILLED") | Some("COMPLETED") => OrderStatus::Closed,
            Some("CANCELED") | Some("CANCELLED") | Some("CANCELED_BY_USER") => {
                OrderStatus::Canceled
            },
            Some("EXPIRED") => OrderStatus::Expired,
            Some("REJECTED") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let amount = order_data
            .quantity
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let price = order_data
            .price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let filled = order_data
            .filled
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = order_data
            .remaining
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok());
        let cost = price.map(|p| filled * p);

        Order {
            id: order_data.order_id.clone().unwrap_or_default(),
            client_order_id: order_data.client_order_id.clone(),
            timestamp: order_data.created_at,
            datetime: order_data.created_at.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: order_data
                .stop_price
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok()),
            trigger_price: None,
            cost,
            trades: vec![],
            fee: None,
            fees: vec![],
            info: serde_json::to_value(order_data).unwrap_or_default(),
            reduce_only: None,
            post_only: None,
            take_profit_price: None,
            stop_loss_price: None,
        }
    }

    fn parse_trade(&self, trade_data: &OxfunTrade, symbol: &str) -> Trade {
        let side_str = trade_data.side.as_deref().unwrap_or("");

        let price = trade_data
            .price
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let amount = trade_data
            .amount
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);

        Trade {
            id: trade_data.id.clone().unwrap_or_default(),
            info: serde_json::to_value(trade_data).unwrap_or_default(),
            timestamp: trade_data.timestamp,
            datetime: trade_data.timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol: symbol.to_string(),
            order: None,
            trade_type: None,
            side: if side_str.is_empty() {
                None
            } else {
                Some(side_str.to_lowercase())
            },
            taker_or_maker: trade_data
                .match_type
                .as_ref()
                .and_then(|s| match s.as_str() {
                    "TAKER" => Some(crate::types::TakerOrMaker::Taker),
                    "MAKER" => Some(crate::types::TakerOrMaker::Maker),
                    _ => None,
                }),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: vec![],
        }
    }

    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        let nonce = Utc::now().timestamp_millis().to_string();
        let body_str = body.unwrap_or("");

        let signature = self.generate_signature(&timestamp, &nonce, method, path, body_str);

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        headers.insert("AccessKey".to_string(), api_key.to_string());
        headers.insert("Timestamp".to_string(), timestamp);
        headers.insert("Signature".to_string(), signature);
        headers.insert("Nonce".to_string(), nonce);

        match method.to_uppercase().as_str() {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => {
                let body_json: Option<serde_json::Value> = if body_str.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(body_str).unwrap_or(serde_json::json!({})))
                };
                self.client.post(path, body_json, Some(headers)).await
            },
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::BadRequest {
                message: format!("Unsupported HTTP method: {method}"),
            }),
        }
    }
}

#[async_trait]
impl Exchange for Oxfun {
    fn id(&self) -> ExchangeId {
        ExchangeId::Oxfun
    }

    fn name(&self) -> &str {
        "OX.FUN"
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
        if let Ok(markets) = self.markets.read() {
            if let Some(market) = markets.get(symbol) {
                return Some(market.id.clone());
            }
        }
        None
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        if let Ok(markets_by_id) = self.markets_by_id.read() {
            if let Some(symbol) = markets_by_id.get(market_id) {
                return Some(symbol.clone());
            }
        }
        None
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let mut request_headers = headers.unwrap_or_default();

        if let Some(api_key) = self.config.api_key() {
            let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
            let nonce = Utc::now().timestamp_millis().to_string();
            let body_str = body.unwrap_or("");

            let signature = self.generate_signature(&timestamp, &nonce, method, path, body_str);

            request_headers.insert("Content-Type".to_string(), "application/json".to_string());
            request_headers.insert("AccessKey".to_string(), api_key.to_string());
            request_headers.insert("Timestamp".to_string(), timestamp);
            request_headers.insert("Signature".to_string(), signature);
            request_headers.insert("Nonce".to_string(), nonce);
        }

        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: request_headers,
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        if !reload {
            let markets = self.markets.read().unwrap();
            if !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let market_list = self.fetch_markets().await?;
        let mut markets = self.markets.write().unwrap();
        let mut markets_by_id = self.markets_by_id.write().unwrap();

        for market in &market_list {
            markets_by_id.insert(market.id.clone(), market.symbol.clone());
            markets.insert(market.symbol.clone(), market.clone());
        }

        Ok(markets.clone())
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: OxfunResponse<Vec<OxfunMarket>> =
            self.public_get("/v3/markets", None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch markets".to_string()),
            });
        }

        let markets_data = response.data.unwrap_or_default();
        let mut markets = Vec::new();

        for market_data in &markets_data {
            let market = self.parse_market(market_data);
            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.safe_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("marketCode".to_string(), market_id);

        let response: OxfunResponse<Vec<OxfunTicker>> =
            self.public_get("/v3/tickers", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch ticker".to_string()),
            });
        }

        let tickers = response.data.unwrap_or_default();
        let ticker_data = tickers.first().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        Ok(self.parse_ticker(ticker_data))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: OxfunResponse<Vec<OxfunTicker>> =
            self.public_get("/v3/tickers", None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch tickers".to_string()),
            });
        }

        let tickers_data = response.data.unwrap_or_default();
        let mut result = HashMap::new();

        for ticker_data in &tickers_data {
            let symbol = self.safe_symbol(&ticker_data.market_code);

            if let Some(filter_symbols) = symbols {
                if !filter_symbols.iter().any(|&s| s == symbol) {
                    continue;
                }
            }

            let ticker = self.parse_ticker(ticker_data);
            result.insert(symbol, ticker);
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.safe_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("marketCode".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("level".to_string(), l.to_string());
        }

        let response: OxfunResponse<OxfunOrderBook> =
            self.public_get("/v3/depth", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch order book".to_string()),
            });
        }

        let ob_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order book data not found".to_string(),
        })?;

        let mut asks = Vec::new();
        let mut bids = Vec::new();

        for ask in &ob_data.asks {
            if ask.len() >= 2 {
                if let (Ok(price), Ok(amount)) =
                    (Decimal::from_str(&ask[0]), Decimal::from_str(&ask[1]))
                {
                    asks.push(OrderBookEntry { price, amount });
                }
            }
        }

        for bid in &ob_data.bids {
            if bid.len() >= 2 {
                if let (Ok(price), Ok(amount)) =
                    (Decimal::from_str(&bid[0]), Decimal::from_str(&bid[1]))
                {
                    bids.push(OrderBookEntry { price, amount });
                }
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp: ob_data.last_updated_at,
            datetime: ob_data.last_updated_at.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.safe_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("marketCode".to_string(), market_id);
        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: OxfunResponse<Vec<OxfunTrade>> =
            self.public_get("/v3/exchange-trades", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch trades".to_string()),
            });
        }

        let trades_data = response.data.unwrap_or_default();

        Ok(trades_data
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.safe_market_id(symbol);
        let tf_str = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("marketCode".to_string(), market_id);
        params.insert("timeframe".to_string(), tf_str.clone());
        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: OxfunResponse<Vec<OxfunCandle>> =
            self.public_get("/v3/candles", Some(params)).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch OHLCV".to_string()),
            });
        }

        let candles_data = response.data.unwrap_or_default();

        Ok(candles_data
            .iter()
            .filter_map(|c| {
                Some(OHLCV {
                    timestamp: c.timestamp?,
                    open: Decimal::from_str(c.open.as_ref()?).ok()?,
                    high: Decimal::from_str(c.high.as_ref()?).ok()?,
                    low: Decimal::from_str(c.low.as_ref()?).ok()?,
                    close: Decimal::from_str(c.close.as_ref()?).ok()?,
                    volume: Decimal::from_str(c.volume.as_ref().or(c.base_volume.as_ref())?)
                        .ok()?,
                })
            })
            .collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: OxfunResponse<Vec<OxfunBalance>> =
            self.private_request("GET", "/v3/balances", None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch balance".to_string()),
            });
        }

        let balances_data = response.data.unwrap_or_default();
        let mut currencies = HashMap::new();

        for balance_data in &balances_data {
            if let Some(ref currency) = balance_data.currency {
                let total = balance_data
                    .total
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let free = balance_data
                    .available
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let used = balance_data
                    .reserved
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);

                currencies.insert(
                    currency.clone(),
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
        }

        Ok(Balances {
            currencies,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            info: serde_json::to_value(&balances_data).unwrap_or_default(),
        })
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = self.safe_market_id(symbol);

        let mut order_params = serde_json::json!({
            "marketCode": market_id,
            "side": match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            },
            "orderType": match order_type {
                OrderType::Limit => "LIMIT",
                OrderType::Market => "MARKET",
                _ => "LIMIT", // Default to LIMIT for unsupported order types
            },
            "quantity": amount.to_string(),
        });

        if let Some(p) = price {
            order_params["price"] = serde_json::json!(p.to_string());
        }

        let body = serde_json::to_string(&serde_json::json!({
            "orders": [order_params]
        }))
        .map_err(|e| CcxtError::ParseError {
            data_type: "OrderParams".to_string(),
            message: e.to_string(),
        })?;

        let response: OxfunResponse<Vec<OxfunOrder>> = self
            .private_request("POST", "/v3/orders/place", Some(&body))
            .await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to create order".to_string()),
            });
        }

        let orders = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order response data not found".to_string(),
        })?;

        let order_data = orders.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Order not found in response".to_string(),
        })?;

        Ok(self.parse_order(order_data))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.safe_market_id(symbol);

        let body = serde_json::to_string(&serde_json::json!({
            "orders": [{
                "marketCode": market_id,
                "orderId": id
            }]
        }))
        .map_err(|e| CcxtError::ParseError {
            data_type: "CancelOrderParams".to_string(),
            message: e.to_string(),
        })?;

        let response: OxfunResponse<Vec<OxfunOrder>> = self
            .private_request("DELETE", "/v3/orders/cancel", Some(&body))
            .await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to cancel order".to_string()),
            });
        }

        let orders = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Cancel response data not found".to_string(),
        })?;

        let order_data = orders.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Canceled order not found in response".to_string(),
        })?;

        Ok(self.parse_order(order_data))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.safe_market_id(symbol);
        let path = format!("/v3/orders/status?marketCode={market_id}&orderId={id}");

        let response: OxfunResponse<Vec<OxfunOrder>> =
            self.private_request("GET", &path, None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch order".to_string()),
            });
        }

        let orders = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order data not found".to_string(),
        })?;

        let order_data = orders.first().ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        Ok(self.parse_order(order_data))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let path = if let Some(s) = symbol {
            let market_id = self.safe_market_id(s);
            format!("/v3/orders/working?marketCode={market_id}")
        } else {
            "/v3/orders/working".to_string()
        };

        let response: OxfunResponse<Vec<OxfunOrder>> =
            self.private_request("GET", &path, None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch open orders".to_string()),
            });
        }

        let orders_data = response.data.unwrap_or_default();
        Ok(orders_data.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut path = "/v3/trades".to_string();
        let mut params_vec = Vec::new();

        if let Some(s) = symbol {
            let market_id = self.safe_market_id(s);
            params_vec.push(format!("marketCode={market_id}"));
        }
        if let Some(s) = since {
            params_vec.push(format!("startTime={s}"));
        }
        if let Some(l) = limit {
            params_vec.push(format!("limit={l}"));
        }

        if !params_vec.is_empty() {
            path = format!("{}?{}", path, params_vec.join("&"));
        }

        let response: OxfunResponse<Vec<OxfunMyTrade>> =
            self.private_request("GET", &path, None).await?;

        if !response.success {
            return Err(CcxtError::ExchangeError {
                message: response
                    .message
                    .unwrap_or_else(|| "Failed to fetch my trades".to_string()),
            });
        }

        let trades_data = response.data.unwrap_or_default();

        Ok(trades_data
            .iter()
            .map(|t| {
                let sym = self.safe_symbol(t.market_code.as_deref().unwrap_or(""));
                let price = t
                    .price
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let amount = t
                    .amount
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                Trade {
                    id: t.trade_id.clone().unwrap_or_default(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                    timestamp: t.timestamp,
                    datetime: t.timestamp.map(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    symbol: sym,
                    order: t.order_id.clone(),
                    trade_type: None,
                    side: t.side.as_ref().map(|s| s.to_lowercase()),
                    taker_or_maker: t.match_type.as_ref().and_then(|s| match s.as_str() {
                        "TAKER" => Some(crate::types::TakerOrMaker::Taker),
                        "MAKER" => Some(crate::types::TakerOrMaker::Maker),
                        _ => None,
                    }),
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: t.fees.as_ref().and_then(|f| {
                        Decimal::from_str(f).ok().map(|amt| crate::types::Fee {
                            currency: t.fee_currency.clone(),
                            cost: Some(amt),
                            rate: None,
                        })
                    }),
                    fees: vec![],
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_exchange() -> Oxfun {
        let config = ExchangeConfig::default()
            .with_api_key("test_key")
            .with_api_secret("test_secret");
        Oxfun::new(config).unwrap()
    }

    #[test]
    fn test_oxfun_creation() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.id(), ExchangeId::Oxfun);
        assert_eq!(exchange.name(), "OX.FUN");
    }

    #[test]
    fn test_has() {
        let exchange = create_test_exchange();
        let features = exchange.has();
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.fetch_trades);
        assert!(features.fetch_ohlcv);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(features.cancel_order);
    }

    #[test]
    fn test_urls() {
        let exchange = create_test_exchange();
        let urls = exchange.urls();
        assert!(urls.api.contains_key("public"));
        assert!(urls.api.contains_key("private"));
        assert_eq!(
            urls.api.get("public"),
            Some(&"https://api.ox.fun".to_string())
        );
    }

    #[test]
    fn test_timeframes() {
        let exchange = create_test_exchange();
        let timeframes = exchange.timeframes();
        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"60".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"3600".to_string()));
    }

    #[test]
    fn test_exchange_info() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.name(), "OX.FUN");
        assert!(exchange
            .urls()
            .doc
            .contains(&"https://docs.ox.fun/".to_string()));
    }

    #[test]
    fn test_parse_order_status() {
        let exchange = create_test_exchange();

        let open_order = OxfunOrder {
            order_id: Some("123".to_string()),
            client_order_id: None,
            market_code: Some("BTC-USD".to_string()),
            side: Some("BUY".to_string()),
            order_type: Some("LIMIT".to_string()),
            quantity: Some("1.0".to_string()),
            price: Some("50000".to_string()),
            stop_price: None,
            filled: Some("0".to_string()),
            remaining: Some("1.0".to_string()),
            status: Some("OPEN".to_string()),
            created_at: Some(1700000000000),
            time_in_force: Some("GTC".to_string()),
        };

        let order = exchange.parse_order(&open_order);
        assert_eq!(order.status, OrderStatus::Open);
        assert_eq!(order.side, OrderSide::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
    }

    #[test]
    fn test_safe_market_id() {
        let exchange = create_test_exchange();

        // Without cached markets, should just replace / with -
        assert_eq!(exchange.safe_market_id("BTC/USD"), "BTC-USD");
        assert_eq!(exchange.safe_market_id("ETH/USDT"), "ETH-USDT");
    }

    #[test]
    fn test_safe_symbol() {
        let exchange = create_test_exchange();

        assert_eq!(exchange.safe_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(exchange.safe_symbol("BTC-USD-SWAP-LIN"), "BTC/USD");
        assert_eq!(exchange.safe_symbol("ETH-USDT"), "ETH/USDT");
    }
}
