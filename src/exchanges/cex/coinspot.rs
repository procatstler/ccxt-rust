//! CoinSpot Exchange Implementation
//!
//! CCXT coinspot.ts ported to Rust
//! Australian cryptocurrency exchange

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// CoinSpot Exchange
pub struct Coinspot {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    nonce: RwLock<i64>,
}

impl Coinspot {
    const PUBLIC_URL: &'static str = "https://www.coinspot.com.au/pubapi";
    const PRIVATE_URL: &'static str = "https://www.coinspot.com.au/api";
    const RATE_LIMIT_MS: u64 = 1000; // 1 second

    /// Create new CoinSpot instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::PUBLIC_URL, &config)?;
        let private_client = HttpClient::new(Self::PRIVATE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: false, // Hardcoded markets
            fetch_currencies: false,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false, // Only limit orders
            cancel_order: true,
            fetch_order: false,
            fetch_orders: false,
            fetch_open_orders: false,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::PUBLIC_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/28208429-3cacdf9a-6896-11e7-854e-4c79a772a30f.jpg".into()),
            api: api_urls,
            www: Some("https://www.coinspot.com.au".into()),
            doc: vec!["https://www.coinspot.com.au/api".into()],
            fees: None,
        };

        Ok(Self {
            config,
            public_client,
            private_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            nonce: RwLock::new(0),
        })
    }

    /// Get nonce for authentication
    fn get_nonce(&self) -> i64 {
        let mut nonce = self.nonce.write().unwrap();
        let new_nonce = Utc::now().timestamp_millis();
        if new_nonce <= *nonce {
            *nonce += 1;
        } else {
            *nonce = new_nonce;
        }
        *nonce
    }

    /// Sign request with HMAC SHA512
    fn sign_request(&self, body: &str) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let mut mac = Hmac::<Sha512>::new_from_slice(secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(body.as_bytes());
        let result = mac.finalize();
        Ok(hex::encode(result.into_bytes()))
    }

    /// Public API call
    async fn public_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.public_client.get(path, None, None).await
    }

    /// Private API call (POST)
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        mut params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;

        // Add nonce
        let nonce = self.get_nonce();
        params.insert("nonce".into(), serde_json::json!(nonce));

        let body = serde_json::to_string(&params)?;
        let signature = self.sign_request(&body)?;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("key".into(), api_key.to_string());
        headers.insert("sign".into(), signature);

        let body_json: serde_json::Value = serde_json::from_str(&body)?;
        self.private_client
            .post(path, Some(body_json), Some(headers))
            .await
    }

    /// Convert market ID to symbol (btc -> BTC/AUD)
    fn to_symbol(market_id: &str) -> String {
        format!("{}/AUD", market_id.to_uppercase())
    }

    /// Convert symbol to market ID (BTC/AUD -> btc)
    fn to_market_id(symbol: &str) -> String {
        symbol.split('/').next().unwrap_or(symbol).to_lowercase()
    }

    /// Get hardcoded markets
    fn get_default_markets() -> Vec<Market> {
        let coins = vec![
            "ADA", "BTC", "ETH", "XRP", "LTC", "DOGE", "RFOX", "POWR", "NEO", "TRX", "EOS", "XLM",
            "GAS",
        ];

        coins
            .into_iter()
            .map(|base| {
                let symbol = format!("{base}/AUD");
                Market {
                    id: base.to_lowercase(),
                    lowercase_id: Some(base.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.to_string(),
                    quote: "AUD".to_string(),
                    base_id: base.to_lowercase(),
                    quote_id: "aud".to_string(),
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
                    taker: Some(Decimal::new(1, 3)), // 0.1%
                    maker: Some(Decimal::new(1, 3)),
                    contract_size: None,
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
            underlying: None,
            underlying_id: None,
                    precision: MarketPrecision {
                        amount: Some(8),
                        price: Some(2),
                        cost: None,
                        base: Some(8),
                        quote: Some(2),
                    },
                    limits: MarketLimits {
                        leverage: MinMax {
                            min: None,
                            max: None,
                        },
                        amount: MinMax {
                            min: None,
                            max: None,
                        },
                        price: MinMax {
                            min: None,
                            max: None,
                        },
                        cost: MinMax {
                            min: None,
                            max: None,
                        },
                    },
                    margin_modes: None,
                    created: None,
                    info: serde_json::json!({}),
                    tier_based: true,
                    percentage: true,
                }
            })
            .collect()
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &CoinspotTicker, symbol: &str) -> Ticker {
        let last = data.last.as_ref().and_then(|v| Decimal::from_str(v).ok());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            high: None,
            low: None,
            bid: data.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: last,
            last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse trade response
    fn parse_trade(&self, data: &CoinspotTrade, symbol: &str) -> Trade {
        let timestamp = data.solddate.or_else(|| {
            data.created.as_ref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.timestamp_millis())
            })
        });

        let price = data
            .rate
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .or_else(|| {
                // Calculate price from total / amount
                let total = data
                    .total
                    .as_ref()
                    .or(data.audtotal.as_ref())
                    .and_then(|v| Decimal::from_str(v).ok());
                let amount = data.amount.as_ref().and_then(|v| Decimal::from_str(v).ok());
                total.zip(amount).and_then(
                    |(t, a)| {
                        if a > Decimal::ZERO {
                            Some(t / a)
                        } else {
                            None
                        }
                    },
                )
            })
            .unwrap_or_default();

        let amount = data
            .amount
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok())
            .unwrap_or_default();

        let cost = data
            .total
            .as_ref()
            .or(data.audtotal.as_ref())
            .and_then(|v| Decimal::from_str(v).ok());

        // Parse fee from audfeeExGst + audGst
        let fee = if let (Some(fee_ex), Some(gst)) = (&data.audfee_ex_gst, &data.aud_gst) {
            let fee_ex_val = Decimal::from_str(fee_ex).ok();
            let gst_val = Decimal::from_str(gst).ok();
            fee_ex_val.zip(gst_val).map(|(f, g)| crate::types::Fee {
                cost: Some(f + g),
                currency: Some("AUD".to_string()),
                rate: None,
            })
        } else {
            None
        };

        Trade {
            id: timestamp.map(|t| t.to_string()).unwrap_or_default(),
            order: None,
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost,
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, response: &CoinspotBalanceResponse) -> Balances {
        let mut result = Balances::new();

        // Handle array format (read-only API)
        if let Some(ref balances) = response.balances {
            for balance_obj in balances {
                for (currency_id, balance_data) in balance_obj {
                    let code = currency_id.to_uppercase();
                    let total = balance_data
                        .balance
                        .as_ref()
                        .and_then(|v| Decimal::from_str(v).ok())
                        .unwrap_or_default();

                    let balance = Balance {
                        free: Some(total),
                        used: Some(Decimal::ZERO),
                        total: Some(total),
                        debt: None,
                    };
                    result.add(&code, balance);
                }
            }
        }

        // Handle object format (read-write API)
        if let Some(ref balance_map) = response.balance {
            for (currency_id, total_str) in balance_map {
                let code = currency_id.to_uppercase();
                let total = Decimal::from_str(total_str).unwrap_or_default();

                let balance = Balance {
                    free: Some(total),
                    used: Some(Decimal::ZERO),
                    total: Some(total),
                    debt: None,
                };
                result.add(&code, balance);
            }
        }

        result
    }
}

#[async_trait]
impl Exchange for Coinspot {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinspot
    }

    fn name(&self) -> &str {
        "CoinSpot"
    }

    fn version(&self) -> &str {
        "1"
    }

    fn countries(&self) -> &[&str] {
        &["AU"]
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

    fn timeframes(&self) -> &HashMap<crate::types::Timeframe, String> {
        static EMPTY: std::sync::OnceLock<HashMap<crate::types::Timeframe, String>> =
            std::sync::OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let markets_vec = Self::get_default_markets();
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
        Ok(Self::get_default_markets())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let response: CoinspotLatestResponse = self.public_get("/latest").await?;

        let market_id = Self::to_market_id(symbol);
        let ticker_data = response
            .prices
            .get(&market_id)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CoinspotLatestResponse = self.public_get("/latest").await?;

        let mut tickers = HashMap::new();

        for (id, data) in &response.prices {
            let symbol = Self::to_symbol(id);

            // Filter by symbols if specified
            if let Some(filter) = symbols {
                if !filter.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let ticker = self.parse_ticker(data, &symbol);
            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = Self::to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("cointype".into(), serde_json::json!(market_id));

        let response: CoinspotOrderBookResponse = self.private_post("/orders", params).await?;

        let bids: Vec<OrderBookEntry> = response
            .buyorders
            .iter()
            .filter_map(|o| {
                let price = Decimal::from_str(&o.rate).ok()?;
                let amount = Decimal::from_str(&o.amount).ok()?;
                Some(OrderBookEntry { price, amount })
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .sellorders
            .iter()
            .filter_map(|o| {
                let price = Decimal::from_str(&o.rate).ok()?;
                let amount = Decimal::from_str(&o.amount).ok()?;
                Some(OrderBookEntry { price, amount })
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
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
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = Self::to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("cointype".into(), serde_json::json!(market_id));

        let response: CoinspotTradesResponse = self.private_post("/orders/history", params).await?;

        let trades: Vec<Trade> = response
            .orders
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        Ok(trades)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();
        let response: CoinspotBalanceResponse = self.private_post("/my/balances", params).await?;
        Ok(self.parse_balance(&response))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        if order_type == OrderType::Market {
            return Err(CcxtError::NotSupported {
                feature: "Market orders not supported on CoinSpot".into(),
            });
        }

        let price = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "CoinSpot createOrder requires a price for limit orders".into(),
        })?;

        let market_id = Self::to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("cointype".into(), serde_json::json!(market_id));
        params.insert("amount".into(), serde_json::json!(amount.to_string()));
        params.insert("rate".into(), serde_json::json!(price.to_string()));

        let path = match side {
            OrderSide::Buy => "/my/buy",
            OrderSide::Sell => "/my/sell",
        };

        let response: CoinspotOrderResponse = self.private_post(path, params).await?;

        Ok(Order {
            id: response.id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
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
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        // CoinSpot requires side parameter to cancel orders
        // Try buy cancel first, then sell cancel
        let mut params = HashMap::new();
        params.insert("id".into(), serde_json::json!(id));

        // Try cancel as buy order
        let buy_result: Result<CoinspotCancelResponse, _> =
            self.private_post("/my/buy/cancel", params.clone()).await;

        if buy_result.is_ok() {
            return Ok(Order {
                id: id.to_string(),
                status: OrderStatus::Canceled,
                info: serde_json::json!({"status": "cancelled"}),
                ..Default::default()
            });
        }

        // Try cancel as sell order
        let sell_result: CoinspotCancelResponse =
            self.private_post("/my/sell/cancel", params).await?;

        Ok(Order {
            id: id.to_string(),
            status: if sell_result.status == "ok" {
                OrderStatus::Canceled
            } else {
                OrderStatus::Open
            },
            info: serde_json::to_value(&sell_result).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();

        if let Some(s) = since {
            let date = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            params.insert("startdate".into(), serde_json::json!(date));
        }

        let response: CoinspotMyTradesResponse =
            self.private_post("/ro/my/transactions", params).await?;

        let mut trades = Vec::new();

        // Parse buy orders
        for mut order in response.buyorders {
            order.side = Some("buy".to_string());
            let sym = order
                .market
                .clone()
                .unwrap_or_else(|| symbol.unwrap_or("").to_string());

            // Filter by symbol if specified
            if let Some(filter_sym) = symbol {
                if sym != filter_sym {
                    continue;
                }
            }

            trades.push(self.parse_trade(&order, &sym));
        }

        // Parse sell orders
        for mut order in response.sellorders {
            order.side = Some("sell".to_string());
            let sym = order
                .market
                .clone()
                .unwrap_or_else(|| symbol.unwrap_or("").to_string());

            // Filter by symbol if specified
            if let Some(filter_sym) = symbol {
                if sym != filter_sym {
                    continue;
                }
            }

            trades.push(self.parse_trade(&order, &sym));
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv not supported on CoinSpot".into(),
        })
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            feature: "fetch_order not supported on CoinSpot".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_open_orders not supported on CoinSpot".into(),
        })
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(Self::to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        _method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let url = if api == "public" {
            format!("{}{}", Self::PUBLIC_URL, path)
        } else {
            format!("{}{}", Self::PRIVATE_URL, path)
        };

        SignedRequest {
            url,
            method: if api == "public" { "GET" } else { "POST" }.into(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

// === CoinSpot API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotTicker {
    bid: Option<String>,
    ask: Option<String>,
    last: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinspotLatestResponse {
    status: Option<String>,
    prices: HashMap<String, CoinspotTicker>,
}

#[derive(Debug, Deserialize)]
struct CoinspotOrderBookEntry {
    amount: String,
    rate: String,
    total: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinspotOrderBookResponse {
    status: Option<String>,
    #[serde(default)]
    buyorders: Vec<CoinspotOrderBookEntry>,
    #[serde(default)]
    sellorders: Vec<CoinspotOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct CoinspotTrade {
    amount: Option<String>,
    rate: Option<String>,
    total: Option<String>,
    coin: Option<String>,
    solddate: Option<i64>,
    market: Option<String>,
    // For my trades
    created: Option<String>,
    #[serde(rename = "audfeeExGst")]
    audfee_ex_gst: Option<String>,
    #[serde(rename = "audGst")]
    aud_gst: Option<String>,
    audtotal: Option<String>,
    otc: Option<bool>,
    #[serde(default)]
    side: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinspotTradesResponse {
    status: Option<String>,
    #[serde(default)]
    orders: Vec<CoinspotTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotBalanceData {
    balance: Option<String>,
    audbalance: Option<String>,
    rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinspotBalanceResponse {
    status: Option<String>,
    // Read-only API format (array of objects)
    balances: Option<Vec<HashMap<String, CoinspotBalanceData>>>,
    // Read-write API format (object with currency keys)
    balance: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotOrderResponse {
    status: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinspotCancelResponse {
    status: String,
}

#[derive(Debug, Deserialize)]
struct CoinspotMyTradesResponse {
    status: Option<String>,
    #[serde(default)]
    buyorders: Vec<CoinspotTrade>,
    #[serde(default)]
    sellorders: Vec<CoinspotTrade>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        assert_eq!(Coinspot::to_market_id("BTC/AUD"), "btc");
        assert_eq!(Coinspot::to_market_id("ETH/AUD"), "eth");
        assert_eq!(Coinspot::to_symbol("btc"), "BTC/AUD");
        assert_eq!(Coinspot::to_symbol("eth"), "ETH/AUD");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let coinspot = Coinspot::new(config).unwrap();

        assert_eq!(coinspot.id(), ExchangeId::Coinspot);
        assert_eq!(coinspot.name(), "CoinSpot");
        assert!(coinspot.has().spot);
        assert!(!coinspot.has().margin);
        assert!(!coinspot.has().swap);
        assert!(coinspot.has().create_order);
        assert!(!coinspot.has().create_market_order);
    }

    #[test]
    fn test_default_markets() {
        let markets = Coinspot::get_default_markets();
        assert!(markets.len() > 10);

        let btc_market = markets.iter().find(|m| m.symbol == "BTC/AUD");
        assert!(btc_market.is_some());
        let btc = btc_market.unwrap();
        assert_eq!(btc.base, "BTC");
        assert_eq!(btc.quote, "AUD");
        assert!(btc.spot);
    }
}
