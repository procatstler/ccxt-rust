//! OceanEx Exchange Implementation
//!
//! CCXT oceanex.ts port to Rust

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// OceanEx exchange
pub struct Oceanex {
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

impl Oceanex {
    const BASE_URL: &'static str = "https://api.oceanex.pro";
    const RATE_LIMIT_MS: u64 = 3000;

    /// Create new OceanEx instance
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
            fetch_my_trades: false,
            fetch_trading_fees: true,
            fetch_time: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("rest".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/58385970-794e2d80-8001-11e9-889c-0567cd79b78e.jpg".into()),
            api: api_urls,
            www: Some("https://www.oceanex.pro.com".into()),
            doc: vec!["https://api.oceanex.pro/doc/v1".into()],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour2, "120".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Hour6, "360".into());
        timeframes.insert(Timeframe::Hour12, "720".into());
        timeframes.insert(Timeframe::Day1, "1440".into());
        timeframes.insert(Timeframe::Day3, "4320".into());
        timeframes.insert(Timeframe::Week1, "10080".into());

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

    /// Private API request
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

        // Build JWT token
        let jwt_token = self.build_jwt(api_key, &params, secret)?;
        let url = format!("{path}?user_jwt={jwt_token}");

        let headers = HashMap::from([
            ("Content-Type".into(), "application/json".into()),
        ]);

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => self.private_client.post(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Build JWT token for authentication
    fn build_jwt(
        &self,
        uid: &str,
        data: &HashMap<String, String>,
        secret: &str,
    ) -> CcxtResult<String> {
        // Simple JWT construction for OceanEx
        // In production, use a proper JWT library
        let header = r#"{"alg":"HS256","typ":"JWT"}"#;
        let payload = serde_json::json!({
            "uid": uid,
            "data": data,
        });

        let header_b64 = URL_SAFE_NO_PAD.encode(header);
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string());
        let message = format!("{}.{}", header_b64, payload_b64);

        let mut hasher = Sha256::new();
        hasher.update(format!("{}{}", message, secret));
        let signature = hasher.finalize();
        let signature_b64 = URL_SAFE_NO_PAD.encode(signature);

        Ok(format!("{}.{}", message, signature_b64))
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &OceanexTickerData, symbol: &str) -> Ticker {
        let ticker = &data.ticker;
        let timestamp = data.at.map(|t| t * 1000);

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            high: ticker.high.clone(),
            low: ticker.low.clone(),
            bid: ticker.buy.clone(),
            bid_volume: None,
            ask: ticker.sell.clone(),
            ask_volume: None,
            vwap: None,
            open: None,
            close: ticker.last.clone(),
            last: ticker.last.clone(),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: ticker.volume.clone(),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &OceanexOrder, symbol: &str) -> Order {
        let status = match data.state.as_str() {
            "wait" => OrderStatus::Open,
            "done" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.ord_type.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.volume.parse().unwrap_or_default();
        let filled: Decimal = data.executed_volume.parse().unwrap_or_default();
        let remaining: Decimal = data.remaining_volume.parse().unwrap_or_default();
        let average: Option<Decimal> = data.avg_price.parse().ok();

        let timestamp = data.created_on.map(|t| t * 1000).or_else(|| {
            data.created_at.as_ref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.timestamp_millis())
            })
        });

        Order {
            id: data.id.to_string(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
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

    /// Parse balance response
    fn parse_balance(&self, accounts: &[OceanexAccount]) -> Balances {
        let mut result = Balances::new();

        for account in accounts {
            let free: Option<Decimal> = account.balance.parse().ok();
            let used: Option<Decimal> = account.locked.parse().ok();
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
            result.add(&account.currency.to_uppercase(), balance);
        }

        result
    }

    /// Parse trade
    fn parse_trade(&self, trade: &OceanexTrade, symbol: &str) -> Trade {
        let side = match trade.side.as_ref().map(|s| s.as_str()) {
            Some("bid") => Some("buy".into()),
            Some("ask") => Some("sell".into()),
            _ => None,
        };

        let timestamp = trade.created_on.map(|t| t * 1000).or_else(|| {
            trade.created_at.as_ref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.timestamp_millis())
            })
        });

        let price: Decimal = trade.price.parse().unwrap_or_default();
        let amount: Decimal = trade.volume.parse().unwrap_or_default();

        Trade {
            id: trade.id.to_string(),
            order: None,
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.to_string(),
            trade_type: Some("limit".into()),
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(trade).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Oceanex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Oceanex
    }

    fn name(&self) -> &str {
        "OceanEx"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["BS"] // Bahamas
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
        let mut params = HashMap::new();
        params.insert("show_details".into(), "true".into());

        let response: OceanexResponse<Vec<OceanexMarket>> = self
            .public_get("/v1/markets", Some(params))
            .await?;

        let mut markets = Vec::new();

        for market_info in response.data {
            if !market_info.enabled {
                continue;
            }

            // Parse symbol name (e.g., "XTZ/USDT")
            let parts: Vec<&str> = market_info.name.split('/').collect();
            if parts.len() != 2 {
                continue;
            }

            let base = parts[0].to_string();
            let quote = parts[1].to_string();
            let symbol = format!("{}/{}", base, quote);

            let price_precision = market_info.price_precision.parse::<i32>().unwrap_or(8);
            let amount_precision = market_info.amount_precision.parse::<i32>().unwrap_or(8);

            let market = Market {
                id: market_info.id.clone(),
                lowercase_id: Some(market_info.id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: parts[0].to_lowercase(),
                quote_id: parts[1].to_lowercase(),
                settle: None,
                settle_id: None,
                active: market_info.enabled,
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
                maker: Some(Decimal::new(1, 3)), // 0.1%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: None,
                    base: Some(amount_precision),
                    quote: Some(price_precision),
                },
                limits: MarketLimits {
                    leverage: MinMax { min: None, max: None },
                    amount: MinMax { min: None, max: None },
                    price: MinMax { min: None, max: None },
                    cost: market_info.minimum_trading_amount.parse().ok().map_or_else(
                        || MinMax { min: None, max: None },
                        |min| MinMax { min: Some(min), max: None },
                    ),
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let response: OceanexResponse<OceanexTickerData> = self
            .public_get(&format!("/v1/tickers/{}", market.id), None)
            .await?;

        Ok(self.parse_ticker(&response.data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market_ids: Vec<String> = if let Some(syms) = symbols {
            syms.iter()
                .filter_map(|s| markets.get(*s).map(|m| m.id.clone()))
                .collect()
        } else {
            markets.values().map(|m| m.id.clone()).collect()
        };

        let mut params = HashMap::new();
        params.insert("markets".into(), market_ids.join(","));

        let response: OceanexResponse<Vec<OceanexTickerMulti>> = self
            .public_get("/v1/tickers_multi", Some(params))
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut tickers = HashMap::new();

        for ticker_data in response.data {
            if let Some(symbol) = markets_by_id.get(&ticker_data.market) {
                let ticker_info = OceanexTickerData {
                    at: ticker_data.at,
                    ticker: ticker_data.ticker,
                };
                let ticker = self.parse_ticker(&ticker_info, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut params = HashMap::new();
        params.insert("market".into(), market.id.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: OceanexResponse<OceanexOrderBook> = self
            .public_get("/v1/order_book", Some(params))
            .await?;

        let data = response.data;
        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(data.timestamp * 1000),
            datetime: chrono::DateTime::from_timestamp(data.timestamp, 0)
                .map(|dt| dt.to_rfc3339()),
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

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut params = HashMap::new();
        params.insert("market".into(), market.id.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: OceanexResponse<Vec<OceanexTrade>> = self
            .public_get("/v1/trades", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| self.parse_trade(t, symbol))
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

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let period = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {:?}", timeframe),
        })?;

        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("market".into(), market.id.clone());
        params.insert("period".into(), period.clone());
        if let Some(s) = since {
            params.insert("timestamp".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(10000).to_string());
        }

        let response: OceanexResponse<Vec<Vec<serde_json::Value>>> = self
            .public_client
            .post("/v1/k", Some(serde_json::to_value(&params).unwrap_or_default()), None)
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .data
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].as_i64()? * 1000,
                    open: c[1].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    high: c[2].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    low: c[3].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    close: c[4].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                    volume: c[5].as_f64().and_then(|v| Decimal::try_from(v).ok())?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        self.load_markets(false).await?;

        let params = HashMap::new();
        let response: OceanexResponse<OceanexAccountInfo> = self
            .private_request("GET", "/v1/members/me", params)
            .await?;

        Ok(self.parse_balance(&response.data.accounts))
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

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        let mut params = HashMap::new();
        params.insert("market".into(), market.id.clone());
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.into());
        params.insert("ord_type".into(), match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {:?}", order_type),
            }),
        }.into());
        params.insert("volume".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: OceanexResponse<OceanexOrder> = self
            .private_request("POST", "/v1/orders", params)
            .await?;

        Ok(self.parse_order(&response.data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());

        let response: OceanexResponse<OceanexOrder> = self
            .private_request("POST", "/v1/order/delete", params)
            .await?;

        Ok(self.parse_order(&response.data, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("ids".into(), format!("[{}]", id));

        let response: OceanexResponse<Vec<OceanexOrder>> = self
            .private_request("GET", "/v1/orders", params)
            .await?;

        let order = response.data.first().ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        Ok(self.parse_order(order, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "OceanEx fetchOpenOrders requires a symbol".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("states".into(), "[\"wait\"]".into());

        self.fetch_orders_with_params(symbol, params).await
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "OceanEx fetchClosedOrders requires a symbol".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("states".into(), "[\"done\",\"cancel\"]".into());

        self.fetch_orders_with_params(symbol, params).await
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let params = HashMap::new();
        let response: OceanexResponse<Vec<OceanexOrder>> = self
            .private_request("POST", "/v1/orders/clear", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| {
                let sym = symbol.unwrap_or_else(|| {
                    markets_by_id
                        .get(&o.market)
                        .map(|s| s.as_str())
                        .unwrap_or("UNKNOWN")
                });
                self.parse_order(o, sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_time(&self) -> CcxtResult<i64> {
        let response: OceanexResponse<i64> = self
            .public_get("/v1/timestamp", None)
            .await?;

        Ok(response.data * 1000) // Convert to milliseconds
    }

    async fn fetch_trading_fees(&self) -> CcxtResult<HashMap<String, crate::types::TradingFee>> {
        let response: OceanexResponse<Vec<OceanexTradingFee>> = self
            .public_get("/v1/fees/trading", None)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut fees = HashMap::new();

        for fee_data in response.data {
            let symbol = markets_by_id
                .get(&fee_data.market)
                .cloned()
                .unwrap_or_else(|| fee_data.market.clone());

            let maker = fee_data.ask_fee.value.parse().unwrap_or_default();
            let taker = fee_data.bid_fee.value.parse().unwrap_or_default();

            fees.insert(
                symbol.clone(),
                crate::types::TradingFee::new(&symbol, maker, taker),
            );
        }

        Ok(fees)
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
        _method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let headers = HashMap::from([
            ("Content-Type".into(), "application/json".into()),
        ]);

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            if let Ok(jwt_token) = self.build_jwt(api_key, params, secret) {
                url = format!("{url}?user_jwt={jwt_token}");
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
            method: "POST".to_string(),
            headers,
            body: None,
        }
    }
}

impl Oceanex {
    async fn fetch_orders_with_params(
        &self,
        symbol: &str,
        mut params: HashMap<String, String>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;

        params.insert("market".into(), market.id.clone());
        params.insert("need_price".into(), "True".into());

        let response: OceanexResponse<Vec<OceanexOrderGroup>> = self
            .private_request("GET", "/v1/orders/filter", params)
            .await?;

        let mut all_orders = Vec::new();
        for group in response.data {
            for order in group.orders {
                all_orders.push(self.parse_order(&order, symbol));
            }
        }

        Ok(all_orders)
    }
}

// === OceanEx API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct OceanexResponse<T> {
    code: i32,
    message: String,
    data: T,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexMarket {
    id: String,
    name: String,
    #[serde(default)]
    enabled: bool,
    price_precision: String,
    amount_precision: String,
    minimum_trading_amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexTickerData {
    #[serde(default)]
    at: Option<i64>,
    ticker: OceanexTickerInfo,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexTickerInfo {
    #[serde(default)]
    buy: Option<Decimal>,
    #[serde(default)]
    sell: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexTickerMulti {
    market: String,
    #[serde(default)]
    at: Option<i64>,
    ticker: OceanexTickerInfo,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexOrderBook {
    timestamp: i64,
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexTrade {
    id: i64,
    price: String,
    volume: String,
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    created_on: Option<i64>,
    #[serde(default)]
    side: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OceanexOrder {
    id: i64,
    market: String,
    side: String,
    ord_type: String,
    price: Option<String>,
    volume: String,
    executed_volume: String,
    remaining_volume: String,
    avg_price: String,
    state: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    created_on: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OceanexAccountInfo {
    accounts: Vec<OceanexAccount>,
}

#[derive(Debug, Deserialize)]
struct OceanexAccount {
    currency: String,
    balance: String,
    locked: String,
}

#[derive(Debug, Deserialize)]
struct OceanexOrderGroup {
    state: String,
    orders: Vec<OceanexOrder>,
}

#[derive(Debug, Deserialize)]
struct OceanexTradingFee {
    market: String,
    ask_fee: OceanexFeeValue,
    bid_fee: OceanexFeeValue,
}

#[derive(Debug, Deserialize)]
struct OceanexFeeValue {
    value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let oceanex = Oceanex::new(config).unwrap();

        assert_eq!(oceanex.id(), ExchangeId::Oceanex);
        assert_eq!(oceanex.name(), "OceanEx");
        assert_eq!(oceanex.version(), "v1");
        assert!(oceanex.has().spot);
        assert!(!oceanex.has().margin);
        assert!(!oceanex.has().swap);
    }
}
