//! Coinbase Exchange (Pro) Implementation
//!
//! Legacy Coinbase Pro/Exchange API implementation

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;
use base64::{Engine as _, engine::general_purpose};

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade,
    Transaction, Fee, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const BASE_URL: &str = "https://api.exchange.coinbase.com";
const RATE_LIMIT_MS: u64 = 100;

/// Coinbase Exchange (Pro) - Legacy API
pub struct CoinbaseExchange {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl CoinbaseExchange {
    /// Create new CoinbaseExchange instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
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
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: false,
            ws: false,
            watch_ticker: false,
            watch_tickers: false,
            watch_order_book: false,
            watch_trades: false,
            watch_ohlcv: false,
            watch_balance: false,
            watch_orders: false,
            watch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), BASE_URL.into());
        api_urls.insert("private".into(), BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/ccxt/ccxt/assets/43336371/34a65553-88aa-4a38-a714-064bd228b97e".into()),
            api: api_urls,
            www: Some("https://coinbase.com/".into()),
            doc: vec![
                "https://docs.cloud.coinbase.com/exchange/docs/".into(),
            ],
            fees: Some("https://docs.pro.coinbase.com/#fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".into());
        timeframes.insert(Timeframe::Minute5, "300".into());
        timeframes.insert(Timeframe::Minute15, "900".into());
        timeframes.insert(Timeframe::Hour1, "3600".into());
        timeframes.insert(Timeframe::Hour6, "21600".into());
        timeframes.insert(Timeframe::Day1, "86400".into());

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

    /// Convert symbol to market ID (BTC/USD -> BTC-USD)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// Convert market ID to symbol (BTC-USD -> BTC/USD)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// Create HMAC signature for authentication
    fn create_signature(&self, timestamp: &str, method: &str, path: &str, body: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let message = format!("{timestamp}{method}{path}{body}");
        let secret_bytes = general_purpose::STANDARD
            .decode(secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError {
                message: format!("Failed to decode secret: {e}"),
            })?;

        let mut mac = HmacSha256::new_from_slice(&secret_bytes)
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        Ok(general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }

    /// Public GET request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private GET request
    async fn private_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let full_path = if params.is_empty() {
            path.to_string()
        } else {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        };

        let signature = self.create_signature(&timestamp, "GET", &full_path, "")?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.get(&full_path, None, Some(headers)).await
    }

    /// Private POST request
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.to_string();
        let signature = self.create_signature(&timestamp, "POST", path, &body_str)?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.post(path, Some(body), Some(headers)).await
    }

    /// Private DELETE request
    async fn private_delete<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let passphrase = self.config.password().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Passphrase required".into(),
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let signature = self.create_signature(&timestamp, "DELETE", path, "")?;

        let mut headers = HashMap::new();
        headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
        headers.insert("CB-ACCESS-SIGN".to_string(), signature);
        headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
        headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        self.client.delete(path, None, Some(headers)).await
    }

    /// Parse market data
    fn parse_market(&self, data: &CoinbaseExchangeProduct) -> Market {
        let base = data.base_currency.clone();
        let quote = data.quote_currency.clone();
        let symbol = format!("{base}/{quote}");

        Market {
            id: data.id.clone(),
            lowercase_id: Some(data.id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: data.base_currency.to_lowercase(),
            quote_id: data.quote_currency.to_lowercase(),
            settle: None,
            settle_id: None,
            market_type: MarketType::Spot,
            spot: true,
            margin: data.margin_enabled.unwrap_or(false),
            swap: false,
            future: false,
            option: false,
            index: false,
            active: data.status.as_ref().map(|s| s == "online").unwrap_or(true),
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: Some(Decimal::new(60, 4)), // 0.006 = 0.6%
            maker: Some(Decimal::new(40, 4)), // 0.004 = 0.4%
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            percentage: true,
            tier_based: true,
            precision: MarketPrecision {
                amount: data.base_increment.as_ref()
                    .and_then(|v| v.parse::<Decimal>().ok())
                    .map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                price: data.quote_increment.as_ref()
                    .and_then(|v| v.parse::<Decimal>().ok())
                    .map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: crate::types::MinMax {
                    min: data.base_min_size.as_ref().and_then(|v| v.parse().ok()),
                    max: data.base_max_size.as_ref().and_then(|v| v.parse().ok()),
                },
                price: crate::types::MinMax {
                    min: data.quote_increment.as_ref().and_then(|v| v.parse().ok()),
                    max: None,
                },
                cost: crate::types::MinMax {
                    min: data.min_market_funds.as_ref().and_then(|v| v.parse().ok()),
                    max: data.max_market_funds.as_ref().and_then(|v| v.parse().ok()),
                },
                leverage: crate::types::MinMax::default(),
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse ticker data
    fn parse_ticker(&self, data: &CoinbaseExchangeTicker, symbol: &str) -> Ticker {
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.bid.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.price.as_ref().and_then(|v| v.parse().ok()),
            last: data.price.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order data
    fn parse_order(&self, data: &CoinbaseExchangeOrder, symbol: &str) -> CcxtResult<Order> {
        let timestamp = data.created_at.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis());

        let status = match data.status.as_deref() {
            Some("pending") | Some("open") | Some("active") => OrderStatus::Open,
            Some("done") | Some("settled") => {
                if data.done_reason.as_deref() == Some("filled") {
                    OrderStatus::Closed
                } else {
                    OrderStatus::Canceled
                }
            }
            Some("rejected") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("market") => OrderType::Market,
            Some("limit") => OrderType::Limit,
            Some("stop") => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|v| v.parse().ok());
        let amount: Decimal = data.size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data.filled_size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Ok(Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
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
            trigger_price: data.stop_price.as_ref().and_then(|v| v.parse().ok()),
            average: data.executed_value.as_ref()
                .and_then(|v| v.parse::<Decimal>().ok())
                .and_then(|executed| {
                    if filled > Decimal::ZERO {
                        Some(executed / filled)
                    } else {
                        None
                    }
                }),
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: data.executed_value.as_ref().and_then(|v| v.parse().ok()),
            trades: Vec::new(),
            reduce_only: None,
            post_only: data.post_only,
            stop_price: data.stop_price.as_ref().and_then(|v| v.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            fee: data.fill_fees.as_ref().and_then(|v| v.parse().ok()).map(|cost| Fee {
                cost: Some(cost),
                currency: None,
                rate: None,
            }),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// Parse trade data
    fn parse_trade_data(&self, data: &CoinbaseExchangeTrade, symbol: &str) -> Trade {
        let timestamp = data.time.as_ref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let price: Decimal = data.price.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.size.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        Trade {
            id: data.trade_id.as_ref().and_then(|id| id.to_string().into()).unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.side.as_ref().map(|s| s.to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance data
    fn parse_balance_data(&self, data: &CoinbaseExchangeAccount) -> (String, Balance) {
        let currency = data.currency.clone().unwrap_or_default();
        let available: Decimal = data.available.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let hold: Decimal = data.hold.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();
        let total: Decimal = data.balance.as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        (
            currency,
            Balance {
                free: Some(available),
                used: Some(hold),
                total: Some(total),
                debt: None,
            },
        )
    }
}

#[async_trait]
impl Exchange for CoinbaseExchange {
    fn id(&self) -> ExchangeId {
        ExchangeId::CoinbaseExchange
    }

    fn name(&self) -> &str {
        "Coinbase Exchange"
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
        self.markets_by_id.read().ok()?.get(symbol).cloned()
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.markets.read().ok()?.get(market_id).map(|m| m.symbol.clone())
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let timestamp = Utc::now().timestamp().to_string();
        let body_str = body.unwrap_or("");

        let message = format!("{timestamp}{method}{path}{body_str}");

        let signature = if let (Some(secret), Some(_api_key), Some(_passphrase)) =
            (self.config.secret(), self.config.api_key(), self.config.password()) {
            if let Ok(secret_bytes) = general_purpose::STANDARD.decode(secret.as_bytes()) {
                let mut mac = HmacSha256::new_from_slice(&secret_bytes)
                    .expect("HMAC can take key of any size");
                mac.update(message.as_bytes());
                general_purpose::STANDARD.encode(mac.finalize().into_bytes())
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let mut new_headers = headers.unwrap_or_default();
        if let (Some(api_key), Some(passphrase)) = (self.config.api_key(), self.config.password()) {
            new_headers.insert("CB-ACCESS-KEY".to_string(), api_key.to_string());
            new_headers.insert("CB-ACCESS-SIGN".to_string(), signature);
            new_headers.insert("CB-ACCESS-TIMESTAMP".to_string(), timestamp);
            new_headers.insert("CB-ACCESS-PASSPHRASE".to_string(), passphrase.to_string());
        }
        new_headers.insert("Content-Type".to_string(), "application/json".to_string());

        let url = if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{}/{}?{}", BASE_URL, path.trim_start_matches('/'), query)
        } else {
            format!("{}/{}", BASE_URL, path.trim_start_matches('/'))
        };

        SignedRequest {
            url,
            method: method.to_string(),
            headers: new_headers,
            body: body.map(|s| s.to_string()),
        }
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: Vec<CoinbaseExchangeProduct> = self
            .public_get("/products", None)
            .await?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for product in &response {
            let market = self.parse_market(product);
            markets_by_id.insert(market.symbol.clone(), market.id.clone());
            markets.insert(market.symbol.clone(), market);
        }

        if let Ok(mut cached) = self.markets.write() {
            *cached = markets.clone();
        }
        if let Ok(mut cached) = self.markets_by_id.write() {
            *cached = markets_by_id;
        }

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let response: CoinbaseExchangeTicker = self
            .public_get(&format!("/products/{market_id}/ticker"), None)
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<CoinbaseExchangeProduct> = self
            .public_get("/products", None)
            .await?;

        let mut tickers = HashMap::new();
        for product in &response {
            let symbol = self.to_symbol(&product.id);
            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }

            // Fetch individual ticker for each product
            if let Ok(ticker_data) = self.fetch_ticker(&symbol).await {
                tickers.insert(symbol, ticker_data);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let _market_id = self.to_market_id(symbol);
        let market_id = _market_id.as_str();
        let mut params = HashMap::new();
        let level = "2";
        params.insert("level".to_string(), level.to_string());

        let response: CoinbaseExchangeOrderBookResponse = self
            .public_get(&format!("/products/{market_id}/book"), Some(params))
            .await?;

        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = response.bids.iter()
            .take(limit.unwrap_or(50) as usize)
            .map(|b| {
                OrderBookEntry {
                    price: b.first().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    amount: b.get(1).and_then(|v| v.parse().ok()).unwrap_or_default(),
                }
            }).collect();

        let asks: Vec<OrderBookEntry> = response.asks.iter()
            .take(limit.unwrap_or(50) as usize)
            .map(|a| {
                OrderBookEntry {
                    price: a.first().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    amount: a.get(1).and_then(|v| v.parse().ok()).unwrap_or_default(),
                }
            }).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: response.sequence,
            bids,
            asks,
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<CoinbaseExchangeTrade> = self
            .public_get(&format!("/products/{market_id}/trades"), Some(params))
            .await?;

        let trades: Vec<Trade> = response.iter().map(|t| {
            self.parse_trade_data(t, symbol)
        }).collect();

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
        let granularity = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("granularity".to_string(), granularity.clone());

        if let Some(s) = since {
            let start = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("start".to_string(), start);
        }

        let response: Vec<Vec<serde_json::Value>> = self
            .public_get(&format!("/products/{market_id}/candles"), Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response.iter()
            .take(limit.unwrap_or(300) as usize)
            .filter_map(|c| {
                if c.len() >= 6 {
                    Some(OHLCV {
                        timestamp: c[0].as_i64().unwrap_or(0) * 1000,
                        open: c[3].as_str().and_then(|v| v.parse().ok()).unwrap_or_default(),
                        high: c[2].as_str().and_then(|v| v.parse().ok()).unwrap_or_default(),
                        low: c[1].as_str().and_then(|v| v.parse().ok()).unwrap_or_default(),
                        close: c[4].as_str().and_then(|v| v.parse().ok()).unwrap_or_default(),
                        volume: c[5].as_str().and_then(|v| v.parse().ok()).unwrap_or_default(),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<CoinbaseExchangeAccount> = self
            .private_get("/accounts", HashMap::new())
            .await?;

        let mut balances = Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
        };

        for account in &response {
            let (currency, balance) = self.parse_balance_data(account);
            balances.currencies.insert(currency, balance);
        }

        Ok(balances)
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

        let mut body = serde_json::json!({
            "product_id": market_id,
            "side": match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            },
            "size": amount.to_string(),
        });

        match order_type {
            OrderType::Market => {
                body["type"] = serde_json::json!("market");
            }
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".into(),
                })?;
                body["type"] = serde_json::json!("limit");
                body["price"] = serde_json::json!(p.to_string());
            }
            _ => {
                return Err(CcxtError::BadRequest {
                    message: format!("Unsupported order type: {order_type:?}"),
                });
            }
        }

        let response: CoinbaseExchangeOrder = self
            .private_post("/orders", body)
            .await?;

        self.parse_order(&response, symbol)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let _response: serde_json::Value = self
            .private_delete(&format!("/orders/{id}"))
            .await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            trigger_price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            cost: None,
            trades: Vec::new(),
            reduce_only: None,
            post_only: None,
            stop_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let response: CoinbaseExchangeOrder = self
            .private_get(&format!("/orders/{id}"), HashMap::new())
            .await?;

        let symbol = self.to_symbol(&response.product_id.clone().unwrap_or_default());
        self.parse_order(&response, &symbol)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), "open".to_string());
        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<CoinbaseExchangeOrder> = self
            .private_get("/orders", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in &response {
            let sym = symbol.map(|s| s.to_string())
                .unwrap_or_else(|| self.to_symbol(&order_data.product_id.clone().unwrap_or_default()));
            orders.push(self.parse_order(order_data, &sym)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), "done".to_string());
        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<CoinbaseExchangeOrder> = self
            .private_get("/orders", params)
            .await?;

        let mut orders = Vec::new();
        for order_data in &response {
            let sym = symbol.map(|s| s.to_string())
                .unwrap_or_else(|| self.to_symbol(&order_data.product_id.clone().unwrap_or_default()));
            orders.push(self.parse_order(order_data, &sym)?);
        }

        Ok(orders)
    }

    async fn fetch_my_trades(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("product_id".to_string(), self.to_market_id(sym));
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<CoinbaseExchangeFill> = self
            .private_get("/fills", params)
            .await?;

        let trades: Vec<Trade> = response.iter().map(|f| {
            let sym = symbol.map(|s| s.to_string())
                .unwrap_or_else(|| self.to_symbol(&f.product_id.clone().unwrap_or_default()));

            let timestamp = f.created_at.as_ref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            let price: Decimal = f.price.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();
            let amount: Decimal = f.size.as_ref()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default();

            Trade {
                id: f.trade_id.as_ref().and_then(|id| id.to_string().into()).unwrap_or_default(),
                order: f.order_id.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: sym,
                trade_type: None,
                side: f.side.as_ref().map(|s| s.to_lowercase()),
                taker_or_maker: f.liquidity.as_ref().map(|l| {
                    if l == "M" {
                        TakerOrMaker::Maker
                    } else {
                        TakerOrMaker::Taker
                    }
                }),
                price,
                amount,
                cost: Some(price * amount),
                fee: f.fee.as_ref().and_then(|v| v.parse().ok()).map(|cost| Fee {
                    cost: Some(cost),
                    currency: None,
                    rate: None,
                }),
                fees: Vec::new(),
                info: serde_json::to_value(f).unwrap_or_default(),
            }
        }).collect();

        Ok(trades)
    }

    async fn fetch_deposits(&self, _code: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Transaction>> {
        Ok(Vec::new())
    }

    async fn fetch_withdrawals(&self, _code: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Transaction>> {
        Ok(Vec::new())
    }
}

// === CoinbaseExchange Response Types ===

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeProduct {
    #[serde(default)]
    id: String,
    #[serde(default)]
    base_currency: String,
    #[serde(default)]
    quote_currency: String,
    #[serde(default)]
    base_increment: Option<String>,
    #[serde(default)]
    quote_increment: Option<String>,
    #[serde(default)]
    base_min_size: Option<String>,
    #[serde(default)]
    base_max_size: Option<String>,
    #[serde(default)]
    min_market_funds: Option<String>,
    #[serde(default)]
    max_market_funds: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    margin_enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeTicker {
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinbaseExchangeOrderBookResponse {
    #[serde(default)]
    sequence: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeTrade {
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeAccount {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    balance: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    hold: Option<String>,
    #[serde(default)]
    profile_id: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default)]
    executed_value: Option<String>,
    #[serde(default)]
    fill_fees: Option<String>,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinbaseExchangeFill {
    #[serde(default)]
    trade_id: Option<i64>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    product_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    liquidity: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let exchange = CoinbaseExchange::new(config).unwrap();

        assert_eq!(exchange.to_market_id("BTC/USD"), "BTC-USD");
        assert_eq!(exchange.to_symbol("BTC-USD"), "BTC/USD");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = CoinbaseExchange::new(config).unwrap();

        assert_eq!(exchange.name(), "Coinbase Exchange");
        assert!(exchange.has().fetch_markets);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().create_order);
    }
}
