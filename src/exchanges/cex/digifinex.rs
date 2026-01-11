//! DigiFinex Exchange Implementation
//!
//! CCXT digifinex.ts port to Rust

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// DigiFinex Exchange
pub struct Digifinex {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Digifinex {
    const BASE_URL: &'static str = "https://openapi.digifinex.com";
    const RATE_LIMIT_MS: u64 = 900; // 900ms between requests

    /// Create new DigiFinex instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: true,
            future: false,
            option: false,
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
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87443315-01283a00-c5fe-11ea-8628-c2a0feaf07ac.jpg".into()),
            api: api_urls,
            www: Some("https://www.digifinex.com".into()),
            doc: vec!["https://docs.digifinex.com".into()],
            fees: Some("https://digifinex.zendesk.com/hc/en-us/articles/360000328422-Fee-Structure-on-DigiFinex".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Hour12, "720".into());
        timeframes.insert(Timeframe::Day1, "1D".into());
        timeframes.insert(Timeframe::Week1, "1W".into());

        Ok(Self {
            config,
            public_client,
            private_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
        })
    }

    /// Public API GET request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let url = if let Some(p) = params {
            let query: String = p
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        self.public_client.get(&url, None, None).await
    }

    /// Private API request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Build signature
        let mut all_params = params.clone();
        all_params.insert("ACCESS-KEY".into(), api_key.to_string());
        all_params.insert("ACCESS-TIMESTAMP".into(), timestamp.clone());

        // Sort params for signature
        let mut sorted_keys: Vec<&String> = all_params.keys().collect();
        sorted_keys.sort();

        let auth_string: String = sorted_keys
            .iter()
            .map(|k| format!("{}={}", k, all_params.get(*k).unwrap()))
            .collect::<Vec<_>>()
            .join("&");

        // Create HMAC-SHA256 signature
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(auth_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-SIGN".into(), signature);
        headers.insert("ACCESS-TIMESTAMP".into(), timestamp);

        let url = if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                headers.insert("Content-Type".into(), "application/json".into());
                let body = if !params.is_empty() {
                    serde_json::to_value(&params).ok()
                } else {
                    None
                };
                self.private_client.post(&url, body, Some(headers)).await
            },
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Symbol to market ID conversion (BTC/USDT -> btc_usdt)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.to_lowercase().replace("/", "_")
    }

    /// Market ID to symbol conversion (btc_usdt -> BTC/USDT)
    fn to_symbol(&self, market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            market_id.to_uppercase()
        }
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &DigifinexTicker, symbol: &str) -> Ticker {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.buy,
            bid_volume: None,
            ask: data.sell,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.change,
            percentage: None,
            average: None,
            base_volume: data.base_vol,
            quote_volume: data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &DigifinexOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            Some("0") => OrderStatus::Open,
            Some("1") => OrderStatus::Open,   // Partially filled
            Some("2") => OrderStatus::Closed, // Filled
            Some("3") => OrderStatus::Canceled,
            Some("4") => OrderStatus::Canceled, // Partially filled and canceled
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = data.price.as_deref().and_then(|p| p.parse().ok());
        let amount: Decimal = data
            .amount
            .as_deref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .executed_amount
            .as_deref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = if let (Some(p), f) = (price, filled) {
            if f > Decimal::ZERO {
                Some(p * f)
            } else {
                None
            }
        } else {
            None
        };

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: data.created_date,
            datetime: data.created_date.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, balances: &[DigifinexBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.free.as_deref().and_then(|f| f.parse().ok());
            let used: Option<Decimal> = b.frozen.as_deref().and_then(|f| f.parse().ok());
            let total = match (free, used) {
                (Some(f), Some(u)) => Some(f + u),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.currency, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Digifinex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Digifinex
    }

    fn name(&self) -> &str {
        "DigiFinex"
    }

    fn version(&self) -> &str {
        "v3"
    }

    fn countries(&self) -> &[&str] {
        &["SG"]
    }

    fn rate_limit(&self) -> u64 {
        Self::RATE_LIMIT_MS
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

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap().clone();
            if !reload && !markets.is_empty() {
                return Ok(markets);
            }
        }

        let markets_vec = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for market in markets_vec {
            markets_by_id.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        {
            let mut markets = self.markets.write().unwrap();
            *markets = markets_map.clone();
        }
        {
            let mut by_id = self.markets_by_id.write().unwrap();
            *by_id = markets_by_id;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: DigifinexMarketsResponse = self.public_get("/v3/spot/symbols", None).await?;

        let mut markets = Vec::new();

        if let Some(symbol_list) = response.symbol_list {
            for symbol_data in symbol_list {
                let parts: Vec<&str> = symbol_data.symbol.split('_').collect();
                if parts.len() != 2 {
                    continue;
                }

                let base = parts[0].to_uppercase();
                let quote = parts[1].to_uppercase();
                let symbol = format!("{base}/{quote}");

                let market = Market {
                    id: symbol_data.symbol.clone(),
                    lowercase_id: Some(symbol_data.symbol.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: parts[0].to_string(),
                    quote_id: parts[1].to_string(),
                    settle: None,
                    settle_id: None,
                    active: true,
                    market_type: MarketType::Spot,
                    spot: true,
                    margin: false,
                    swap: false,
                    future: false,
                    option: false,
                    index: false,
                    contract: false,
                    linear: None,
                    inverse: None,
                    sub_type: None,
                    taker: Some(Decimal::new(2, 3)), // 0.002 = 0.2%
                    maker: Some(Decimal::new(2, 3)), // 0.002 = 0.2%
                    contract_size: None,
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
                    precision: MarketPrecision {
                        amount: Some(symbol_data.amount_precision.unwrap_or(8)),
                        price: Some(symbol_data.price_precision.unwrap_or(8)),
                        cost: None,
                        base: Some(symbol_data.amount_precision.unwrap_or(8)),
                        quote: Some(symbol_data.price_precision.unwrap_or(8)),
                    },
                    limits: MarketLimits::default(),
                    margin_modes: None,
                    created: None,
                    info: serde_json::to_value(&symbol_data).unwrap_or_default(),
                    tier_based: true,
                    percentage: true,
                };

                markets.push(market);
            }
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: DigifinexTickerResponse = self.public_get("/v3/ticker", Some(params)).await?;

        if let Some(ticker_data) = response.ticker.first() {
            Ok(self.parse_ticker(ticker_data, symbol))
        } else {
            Err(CcxtError::ExchangeError {
                message: "No ticker data returned".into(),
            })
        }
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: DigifinexTickerResponse = self.public_get("/v3/ticker", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut tickers = HashMap::new();

        for ticker_data in response.ticker {
            if let Some(symbol) = markets_by_id.get(&ticker_data.symbol) {
                // Filter by symbols if specified
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&ticker_data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: DigifinexOrderBookResponse =
            self.public_get("/v3/order_book", Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price,
                amount: b.amount,
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price,
                amount: a.amount,
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: response.date,
            datetime: response.date.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: DigifinexTradesResponse = self.public_get("/v3/trades", Some(params)).await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| {
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.amount.parse().unwrap_or_default();

                Trade {
                    id: t.id.clone(),
                    order: None,
                    timestamp: Some(t.date),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.date)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.trade_type.clone()),
                    taker_or_maker: if t.trade_type == "buy" {
                        Some(TakerOrMaker::Taker)
                    } else {
                        Some(TakerOrMaker::Maker)
                    },
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
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
        let market_id = self.to_market_id(symbol);

        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("period".into(), interval.clone());

        if let Some(l) = limit {
            params.insert("limit".into(), l.min(500).to_string());
        }

        if let Some(s) = since {
            params.insert("start_time".into(), (s / 1000).to_string());
        }

        let response: DigifinexKlineResponse = self.public_get("/v3/kline", Some(params)).await?;

        let ohlcv_list: Vec<OHLCV> = response
            .data
            .iter()
            .map(|k| {
                OHLCV {
                    timestamp: k[0] as i64 * 1000, // Convert to milliseconds
                    open: Decimal::from_f64_retain(k[1]).unwrap_or_default(),
                    high: Decimal::from_f64_retain(k[3]).unwrap_or_default(),
                    low: Decimal::from_f64_retain(k[4]).unwrap_or_default(),
                    close: Decimal::from_f64_retain(k[2]).unwrap_or_default(),
                    volume: Decimal::from_f64_retain(k[5]).unwrap_or_default(),
                }
            })
            .collect();

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: DigifinexBalanceResponse = self
            .private_request("GET", "/v3/spot/assets", params)
            .await?;

        Ok(self.parse_balance(&response.list))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert(
            "type".into(),
            match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }
            .into(),
        );
        params.insert("amount".into(), amount.to_string());

        match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("price".into(), price_val.to_string());
            },
            OrderType::Market => {
                // Market orders don't need price
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                });
            },
        }

        let response: DigifinexOrderResponse = self
            .private_request("POST", "/v3/spot/order/new", params)
            .await?;

        // Fetch the created order details
        let mut fetch_params = HashMap::new();
        fetch_params.insert("order_id".into(), response.order_id.to_string());

        let order_response: DigifinexOrderDetailResponse = self
            .private_request("GET", "/v3/spot/order", fetch_params)
            .await?;

        Ok(self.parse_order(&order_response.data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let _market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let _response: DigifinexCancelResponse = self
            .private_request("POST", "/v3/spot/order/cancel", params)
            .await?;

        // Fetch the canceled order details
        let mut fetch_params = HashMap::new();
        fetch_params.insert("order_id".into(), id.to_string());

        let order_response: DigifinexOrderDetailResponse = self
            .private_request("GET", "/v3/spot/order", fetch_params)
            .await?;

        Ok(self.parse_order(&order_response.data, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: DigifinexOrderDetailResponse = self
            .private_request("GET", "/v3/spot/order", params)
            .await?;

        Ok(self.parse_order(&response.data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: DigifinexOrdersResponse = self
            .private_request("GET", "/v3/spot/order/current", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol.clone().unwrap_or_default())
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone().unwrap_or_default());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: DigifinexOrdersResponse = self
            .private_request("GET", "/v3/spot/order/history", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol.clone().unwrap_or_default())
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone().unwrap_or_default());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol_str = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "DigiFinex fetchMyTrades requires a symbol".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), self.to_market_id(symbol_str));

        if let Some(l) = limit {
            params.insert("limit".into(), l.min(500).to_string());
        }

        let response: DigifinexMyTradesResponse = self
            .private_request("GET", "/v3/spot/mytrades", params)
            .await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| {
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.amount.parse().unwrap_or_default();
                let cost = price * amount;

                Trade {
                    id: t.id.clone(),
                    order: Some(t.order_id.clone()),
                    timestamp: Some(t.timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol_str.to_string(),
                    trade_type: None,
                    side: Some(t.side.clone()),
                    taker_or_maker: if t.is_maker {
                        Some(TakerOrMaker::Maker)
                    } else {
                        Some(TakerOrMaker::Taker)
                    },
                    price,
                    amount,
                    cost: Some(cost),
                    fee: Some(crate::types::Fee {
                        cost: t.fee.parse().ok(),
                        currency: Some(t.fee_currency.clone()),
                        rate: None,
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let timestamp = Utc::now().timestamp_millis().to_string();

            let mut all_params = params.clone();
            all_params.insert("ACCESS-KEY".into(), api_key.to_string());
            all_params.insert("ACCESS-TIMESTAMP".into(), timestamp.clone());

            let mut sorted_keys: Vec<&String> = all_params.keys().collect();
            sorted_keys.sort();

            let auth_string: String = sorted_keys
                .iter()
                .map(|k| format!("{}={}", k, all_params.get(*k).unwrap()))
                .collect::<Vec<_>>()
                .join("&");

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(auth_string.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            headers.insert("ACCESS-KEY".into(), api_key.to_string());
            headers.insert("ACCESS-SIGN".into(), signature);
            headers.insert("ACCESS-TIMESTAMP".into(), timestamp);

            if method == "GET" && !params.is_empty() {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                url = format!("{url}?{query}");
            }
        } else if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// === DigiFinex API Response Types ===

#[derive(Debug, Deserialize)]
struct DigifinexMarketsResponse {
    code: i32,
    symbol_list: Option<Vec<DigifinexSymbol>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DigifinexSymbol {
    symbol: String,
    #[serde(default)]
    amount_precision: Option<i32>,
    #[serde(default)]
    price_precision: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DigifinexTicker {
    symbol: String,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    buy: Option<Decimal>,
    #[serde(default)]
    sell: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    base_vol: Option<Decimal>,
    #[serde(default)]
    change: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DigifinexTickerResponse {
    ticker: Vec<DigifinexTicker>,
}

#[derive(Debug, Deserialize)]
struct DigifinexOrderBookResponse {
    #[serde(default)]
    date: Option<i64>,
    bids: Vec<DigifinexOrderBookLevel>,
    asks: Vec<DigifinexOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct DigifinexOrderBookLevel {
    price: Decimal,
    amount: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct DigifinexTrade {
    id: String,
    date: i64,
    price: String,
    amount: String,
    #[serde(rename = "type")]
    trade_type: String,
}

#[derive(Debug, Deserialize)]
struct DigifinexTradesResponse {
    data: Vec<DigifinexTrade>,
}

#[derive(Debug, Deserialize)]
struct DigifinexKlineResponse {
    data: Vec<Vec<f64>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DigifinexOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    executed_amount: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "type")]
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    created_date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DigifinexOrderResponse {
    order_id: String,
}

#[derive(Debug, Deserialize)]
struct DigifinexOrderDetailResponse {
    data: DigifinexOrder,
}

#[derive(Debug, Deserialize)]
struct DigifinexOrdersResponse {
    data: Vec<DigifinexOrder>,
}

#[derive(Debug, Deserialize)]
struct DigifinexCancelResponse {
    success: bool,
}

#[derive(Debug, Deserialize)]
struct DigifinexBalance {
    currency: String,
    #[serde(default)]
    free: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DigifinexBalanceResponse {
    list: Vec<DigifinexBalance>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DigifinexMyTrade {
    id: String,
    order_id: String,
    timestamp: i64,
    price: String,
    amount: String,
    side: String,
    fee: String,
    fee_currency: String,
    is_maker: bool,
}

#[derive(Debug, Deserialize)]
struct DigifinexMyTradesResponse {
    data: Vec<DigifinexMyTrade>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let digifinex = Digifinex::new(config).unwrap();

        assert_eq!(digifinex.to_market_id("BTC/USDT"), "btc_usdt");
        assert_eq!(digifinex.to_market_id("ETH/BTC"), "eth_btc");
        assert_eq!(digifinex.to_symbol("btc_usdt"), "BTC/USDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let digifinex = Digifinex::new(config).unwrap();

        assert_eq!(digifinex.id(), ExchangeId::Digifinex);
        assert_eq!(digifinex.name(), "DigiFinex");
        assert!(digifinex.has().spot);
        assert!(digifinex.has().margin);
        assert!(digifinex.has().swap);
    }
}
