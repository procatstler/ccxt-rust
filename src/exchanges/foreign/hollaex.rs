//! Hollaex Exchange Implementation
//!
//! CCXT hollaex.ts port to Rust

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Hollaex exchange
pub struct Hollaex {
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

impl Hollaex {
    const BASE_URL: &'static str = "https://api.hollaex.com";
    const RATE_LIMIT_MS: u64 = 250; // 4 requests per second => 1000ms / 4 = 250ms

    /// Create new Hollaex instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_order_books: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            create_post_only_order: true,
            create_stop_order: true,
            cancel_order: true,
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_trading_fees: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: false,
            fetch_deposit_addresses: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/75841031-ca375180-5ddd-11ea-8417-b975674c23cb.jpg".into()),
            api: api_urls,
            www: Some("https://hollaex.com".into()),
            doc: vec!["https://apidocs.hollaex.com".into()],
            fees: Some("https://hollaex.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());

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

    /// Public API call
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

    /// Private API call
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        // Calculate expiration (default 60 seconds from now)
        let expires = Utc::now().timestamp() + 60;
        let expires_str = expires.to_string();

        // Build authentication string
        let auth_str = if method == "POST" && !params.is_empty() {
            let body = serde_json::to_string(&params).unwrap_or_default();
            format!("{method}{path}{expires_str}{body}")
        } else {
            format!("{method}{path}{expires_str}")
        };

        // Create HMAC-SHA256 signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(auth_str.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("api-key".into(), api_key.to_string());
        headers.insert("api-signature".into(), signature);
        headers.insert("api-expires".into(), expires_str);

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
                    Some(serde_json::to_value(&params).unwrap_or_default())
                } else {
                    None
                };
                self.private_client.post(&url, body, Some(headers)).await
            }
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &HollaexTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp.as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis())
            .or_else(|| Some(Utc::now().timestamp_millis()));

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: data.timestamp.clone(),
            high: data.high,
            low: data.low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.open,
            close: data.close,
            last: data.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &HollaexOrder, symbol: &str) -> Order {
        let status = match data.status.as_str() {
            "new" => OrderStatus::Open,
            "pfilled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data.price;
        let amount: Decimal = data.size;
        let filled: Decimal = data.filled;
        let remaining = Some(amount - filled);

        let cost = if filled > Decimal::ZERO {
            price.map(|p| p * filled)
        } else {
            None
        };

        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        let timestamp = data.created_at.as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let updated_timestamp = data.updated_at.as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        // Check for post-only meta flag
        let post_only = data.meta.as_ref()
            .and_then(|m| m.get("post_only"))
            .and_then(|v| v.as_bool());

        Order {
            id: data.id.clone(),
            client_order_id: None,
            timestamp,
            datetime: data.created_at.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: updated_timestamp,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price: data.stop,
            trigger_price: data.stop,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, data: &HollaexBalanceResponse) -> Balances {
        let mut result = Balances::new();

        // Extract all currency balances from the response
        // The response has fields like btc_balance, btc_available, etc.
        let balance_map = serde_json::to_value(data).unwrap_or_default();
        if let Some(obj) = balance_map.as_object() {
            let mut processed_currencies = std::collections::HashSet::new();

            for (key, _value) in obj {
                // Extract currency code from keys like "btc_balance"
                if let Some(currency) = key.strip_suffix("_balance") {
                    if processed_currencies.contains(currency) {
                        continue;
                    }
                    processed_currencies.insert(currency.to_string());

                    let free_key = format!("{}_available", currency);
                    let total_key = format!("{}_balance", currency);

                    let free = obj.get(&free_key)
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<Decimal>().ok());

                    let total = obj.get(&total_key)
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<Decimal>().ok());

                    let used = match (total, free) {
                        (Some(t), Some(f)) => Some(t - f),
                        _ => None,
                    };

                    let balance = Balance {
                        free,
                        used,
                        total,
                        debt: None,
                    };

                    result.add(&currency.to_uppercase(), balance);
                }
            }
        }

        result
    }

    /// Parse trade response
    fn parse_trade(&self, data: &HollaexTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp.as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let price: Decimal = data.price;
        let amount: Decimal = data.size;
        let cost = price * amount;

        let fee = data.fee.map(|f| crate::types::Fee {
            cost: Some(f),
            currency: data.fee_coin.clone(),
            rate: None,
        });

        Trade {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp,
            datetime: data.timestamp.clone(),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(data.side.clone()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(cost),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Hollaex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Hollaex
    }

    fn name(&self) -> &str {
        "HollaEx"
    }

    fn version(&self) -> &str {
        "v2"
    }

    fn countries(&self) -> &[&str] {
        &["KR"]
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
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
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
        let response: HollaexConstants = self
            .public_get("/v2/constants", None)
            .await?;

        let mut markets = Vec::new();

        for (pair_id, pair_data) in response.pairs {
            let base = pair_data.pair_base.to_uppercase();
            let quote = pair_data.pair_2.to_uppercase();
            let symbol = format!("{}/{}", base, quote);

            let market = Market {
                id: pair_id.clone(),
                lowercase_id: Some(pair_id.clone()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: pair_data.pair_base.clone(),
                quote_id: pair_data.pair_2.clone(),
                settle: None,
                settle_id: None,
                active: pair_data.active,
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
                taker: pair_data.taker_fees.get("1").copied(),
                maker: pair_data.maker_fees.get("1").copied(),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(pair_data.increment_size.to_string().split('.').nth(1).map(|s| s.len() as i32).unwrap_or(8)),
                    price: Some(pair_data.increment_price.to_string().split('.').nth(1).map(|s| s.len() as i32).unwrap_or(8)),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: Some(pair_data.min_size),
                        max: Some(pair_data.max_size),
                    },
                    price: crate::types::MinMax {
                        min: Some(pair_data.min_price),
                        max: Some(pair_data.max_price),
                    },
                    cost: crate::types::MinMax { min: None, max: None },
                    leverage: crate::types::MinMax { min: None, max: None },
                },
                margin_modes: None,
                created: pair_data.created_at.as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.timestamp_millis()),
                info: serde_json::to_value(&pair_data).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: HollaexTicker = self
            .public_get("/v2/ticker", Some(params))
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: HashMap<String, HollaexTicker> = self
            .public_get("/v2/tickers", None)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for (market_id, data) in response {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id.clone());

        let response: HashMap<String, HollaexOrderBookData> = self
            .public_get("/v2/orderbook", Some(params))
            .await?;

        let orderbook_data = response.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("No orderbook data for {}", market_id),
        })?;

        let mut bids: Vec<OrderBookEntry> = orderbook_data.bids
            .iter()
            .take(limit.unwrap_or(u32::MAX) as usize)
            .map(|b| OrderBookEntry {
                price: b[0],
                amount: b[1],
            })
            .collect();

        let mut asks: Vec<OrderBookEntry> = orderbook_data.asks
            .iter()
            .take(limit.unwrap_or(u32::MAX) as usize)
            .map(|a| OrderBookEntry {
                price: a[0],
                amount: a[1],
            })
            .collect();

        // Ensure proper sorting
        bids.sort_by(|a, b| b.price.cmp(&a.price)); // Descending
        asks.sort_by(|a, b| a.price.cmp(&b.price)); // Ascending

        let timestamp = orderbook_data.timestamp.as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: orderbook_data.timestamp.clone(),
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
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id.clone());

        let response: HashMap<String, Vec<HollaexPublicTrade>> = self
            .public_get("/v2/trades", Some(params))
            .await?;

        let trades_data = response.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("No trades data for {}", market_id),
        })?;

        let mut trades: Vec<Trade> = trades_data
            .iter()
            .take(limit.unwrap_or(u32::MAX) as usize)
            .map(|t| {
                let timestamp = t.timestamp.as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.timestamp_millis());

                let price: Decimal = t.price;
                let amount: Decimal = t.size;
                let cost = price * amount;

                Trade {
                    id: String::new(),
                    order: None,
                    timestamp,
                    datetime: t.timestamp.clone(),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.side.clone()),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(cost),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        trades.reverse(); // Most recent first

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let resolution = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {:?}", timeframe),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("resolution".into(), resolution.clone());

        // Calculate time range
        let limit_val = limit.unwrap_or(500).min(500);
        let timeframe_ms = match timeframe {
            Timeframe::Minute1 => 60_000,
            Timeframe::Minute5 => 300_000,
            Timeframe::Minute15 => 900_000,
            Timeframe::Hour1 => 3_600_000,
            Timeframe::Hour4 => 14_400_000,
            Timeframe::Day1 => 86_400_000,
            Timeframe::Week1 => 604_800_000,
            _ => 60_000,
        };

        let to = Utc::now().timestamp();
        let from = since.map(|s| s / 1000).unwrap_or(to - (timeframe_ms * limit_val as i64) / 1000);

        params.insert("from".into(), from.to_string());
        params.insert("to".into(), to.to_string());

        let response: Vec<HollaexOHLCV> = self
            .public_get("/v2/chart", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                let timestamp = c.time.as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.timestamp_millis())?;

                Some(OHLCV {
                    timestamp,
                    open: c.open,
                    high: c.high,
                    low: c.low,
                    close: c.close,
                    volume: c.volume,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: HollaexBalanceResponse = self
            .private_request("GET", "/v2/user/balance", HashMap::new())
            .await?;

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
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol)
                .map(|m| m.id.clone())
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })?
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.into());
        params.insert("size".into(), amount.to_string());
        params.insert("type".into(), match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {:?}", order_type),
            }),
        }.into());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: HollaexOrder = self
            .private_request("POST", "/v2/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: HollaexOrder = self
            .private_request("DELETE", "/v2/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: HollaexOrder = self
            .private_request("GET", "/v2/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets.get(s)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: s.to_string(),
                    })?
            };
            params.insert("symbol".into(), market_id);
        }

        if let Some(s) = since {
            let start_date = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start_date".into(), start_date);
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: HollaexOrdersResponse = self
            .private_request("GET", "/v2/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let orders: Vec<Order> = response.data
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("open".into(), "true".into());

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets.get(s)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: s.to_string(),
                    })?
            };
            params.insert("symbol".into(), market_id);
        }

        if let Some(s) = since {
            let start_date = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start_date".into(), start_date);
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: HollaexOrdersResponse = self
            .private_request("GET", "/v2/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let orders: Vec<Order> = response.data
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("open".into(), "false".into());

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets.get(s)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: s.to_string(),
                    })?
            };
            params.insert("symbol".into(), market_id);
        }

        if let Some(s) = since {
            let start_date = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start_date".into(), start_date);
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: HollaexOrdersResponse = self
            .private_request("GET", "/v2/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let orders: Vec<Order> = response.data
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                markets.get(s)
                    .map(|m| m.id.clone())
                    .ok_or_else(|| CcxtError::BadSymbol {
                        symbol: s.to_string(),
                    })?
            };
            params.insert("symbol".into(), market_id);
        }

        if let Some(s) = since {
            let start_date = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start_date".into(), start_date);
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: HollaexTradesResponse = self
            .private_request("GET", "/v2/user/trades", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let trades: Vec<Trade> = response.data
            .iter()
            .map(|t| {
                let sym = symbol
                    .map(|s| s.to_string())
                    .or_else(|| markets_by_id.get(&t.symbol.clone().unwrap_or_default()).cloned())
                    .unwrap_or_default();
                self.parse_trade(t, &sym)
            })
            .collect();

        Ok(trades)
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

            let expires = Utc::now().timestamp() + 60;
            let expires_str = expires.to_string();

            let auth_str = if method == "POST" && !params.is_empty() {
                let body = serde_json::to_string(&params).unwrap_or_default();
                format!("{method}{path}{expires_str}{body}")
            } else {
                format!("{method}{path}{expires_str}")
            };

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(auth_str.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            headers.insert("api-key".into(), api_key.to_string());
            headers.insert("api-signature".into(), signature);
            headers.insert("api-expires".into(), expires_str);
            headers.insert("Content-Type".into(), "application/json".into());

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
            body: if method == "POST" && !params.is_empty() {
                Some(serde_json::to_string(&params).unwrap_or_default())
            } else {
                None
            },
        }
    }
}

// === Hollaex API Response Types ===

#[derive(Debug, Deserialize)]
struct HollaexConstants {
    coins: HashMap<String, HollaexCoin>,
    pairs: HashMap<String, HollaexPair>,
}

#[derive(Debug, Deserialize)]
struct HollaexCoin {
    id: i64,
    fullname: String,
    symbol: String,
    active: bool,
    #[serde(default)]
    allow_deposit: bool,
    #[serde(default)]
    allow_withdrawal: bool,
    #[serde(default)]
    withdrawal_fee: Option<Decimal>,
    min: Decimal,
    max: Decimal,
    increment_unit: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct HollaexPair {
    id: i64,
    name: String,
    pair_base: String,
    pair_2: String,
    #[serde(default)]
    taker_fees: HashMap<String, Decimal>,
    #[serde(default)]
    maker_fees: HashMap<String, Decimal>,
    min_size: Decimal,
    max_size: Decimal,
    min_price: Decimal,
    max_price: Decimal,
    increment_size: Decimal,
    increment_price: Decimal,
    active: bool,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HollaexTicker {
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HollaexOrderBookData {
    bids: Vec<[Decimal; 2]>,
    asks: Vec<[Decimal; 2]>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HollaexPublicTrade {
    size: Decimal,
    price: Decimal,
    side: String,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HollaexTrade {
    #[serde(default)]
    id: Option<i64>,
    side: String,
    #[serde(default)]
    symbol: Option<String>,
    size: Decimal,
    price: Decimal,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    fee: Option<Decimal>,
    #[serde(default)]
    fee_coin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HollaexOHLCV {
    #[serde(default)]
    time: Option<String>,
    close: Decimal,
    high: Decimal,
    low: Decimal,
    open: Decimal,
    volume: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct HollaexOrder {
    id: String,
    side: String,
    symbol: String,
    size: Decimal,
    filled: Decimal,
    #[serde(default)]
    stop: Option<Decimal>,
    #[serde(default)]
    fee: Option<Decimal>,
    #[serde(default)]
    fee_coin: Option<String>,
    #[serde(rename = "type")]
    order_type: String,
    #[serde(default)]
    price: Option<Decimal>,
    status: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    meta: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct HollaexOrdersResponse {
    count: i32,
    data: Vec<HollaexOrder>,
}

#[derive(Debug, Deserialize)]
struct HollaexTradesResponse {
    count: i32,
    data: Vec<HollaexTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HollaexBalanceResponse {
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(flatten)]
    balances: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let hollaex = Hollaex::new(config).unwrap();

        assert_eq!(hollaex.id(), ExchangeId::Hollaex);
        assert_eq!(hollaex.name(), "HollaEx");
        assert!(hollaex.has().spot);
        assert!(!hollaex.has().margin);
        assert!(!hollaex.has().swap);
    }
}
