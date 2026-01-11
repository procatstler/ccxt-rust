//! Bitbns Exchange Implementation
//!
//! Indian cryptocurrency exchange (<https://bitbns.com>)

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::Sha512;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Fee, OHLCV,
};

const WWW_URL: &str = "https://bitbns.com";
const V1_URL: &str = "https://api.bitbns.com/api/trade/v1";
const V2_URL: &str = "https://api.bitbns.com/api/trade/v2";
const RATE_LIMIT_MS: u64 = 1000;

/// Bitbns Exchange
pub struct Bitbns {
    config: ExchangeConfig,
    www_client: HttpClient,
    v1_client: HttpClient,
    v2_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// API Response Types
#[derive(Debug, Deserialize)]
struct BitbnsMarket {
    id: Option<String>,
    symbol: Option<String>,
    base: Option<String>,
    quote: Option<String>,
    #[serde(rename = "baseId")]
    base_id: Option<String>,
    active: Option<bool>,
    limits: Option<BitbnsLimits>,
    precision: Option<BitbnsPrecision>,
}

#[derive(Debug, Deserialize)]
struct BitbnsLimits {
    amount: Option<BitbnsMinMax>,
    price: Option<BitbnsMinMax>,
    cost: Option<BitbnsMinMax>,
}

#[derive(Debug, Deserialize)]
struct BitbnsMinMax {
    min: Option<f64>,
    max: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BitbnsPrecision {
    amount: Option<i32>,
    price: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct BitbnsTicker {
    symbol: Option<String>,
    timestamp: Option<i64>,
    high: Option<String>,
    low: Option<String>,
    bid: Option<f64>,
    ask: Option<f64>,
    open: Option<f64>,
    close: Option<f64>,
    last: Option<f64>,
    #[serde(rename = "baseVolume")]
    base_volume: Option<f64>,
    change: Option<f64>,
    percentage: Option<f64>,
    average: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BitbnsOrderBook {
    bids: Option<Vec<Vec<f64>>>,
    asks: Option<Vec<Vec<f64>>>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BitbnsApiResponse<T> {
    data: Option<T>,
    status: Option<i32>,
    error: Option<String>,
    code: Option<i32>,
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BitbnsTrade {
    #[serde(rename = "tradeId")]
    trade_id: Option<String>,
    price: Option<String>,
    quote_volume: Option<f64>,
    base_volume: Option<f64>,
    timestamp: Option<i64>,
    #[serde(rename = "type")]
    trade_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitbnsOrder {
    entry_id: Option<i64>,
    btc: Option<f64>,
    rate: Option<f64>,
    time: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<i32>,
    status: Option<i32>,
    t_rate: Option<f64>,
}

impl Bitbns {
    /// Create new Bitbns instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let www_client = HttpClient::new(WWW_URL, &config)?;
        let v1_client = HttpClient::new(V1_URL, &config)?;
        let v2_client = HttpClient::new(V2_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("www".into(), WWW_URL.into());
        api_urls.insert("v1".into(), V1_URL.into());
        api_urls.insert("v2".into(), V2_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/a5b9a562-cdd8-4bea-9fa7-fd24c1dad3d9".into()),
            api: api_urls,
            www: Some("https://bitbns.com".into()),
            doc: vec!["https://bitbns.com/trade/#/api-trading/".into()],
            fees: Some("https://bitbns.com/fees".into()),
        };

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
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: false,
            fetch_deposit_address: true,
            ws: false,
            ..Default::default()
        };

        let timeframes = HashMap::new();

        Ok(Self {
            config,
            www_client,
            v1_client,
            v2_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
        })
    }

    /// Get uppercase ID for API calls
    fn get_uppercase_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            // Check if it's a USDT market
            if market.quote == "USDT" {
                Ok(format!("{}_{}", market.base, market.quote))
            } else {
                Ok(market.base.clone())
            }
        } else {
            // Parse symbol directly
            let parts: Vec<&str> = symbol.split('/').collect();
            if parts.len() == 2 {
                if parts[1] == "USDT" {
                    Ok(format!("{}_{}", parts[0], parts[1]))
                } else {
                    Ok(parts[0].to_string())
                }
            } else {
                Err(CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })
            }
        }
    }

    /// HMAC-SHA512 signature
    fn create_signature(&self, payload: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".to_string(),
        })?;

        type HmacSha512 = Hmac<Sha512>;
        let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            })?;
        mac.update(payload.as_bytes());

        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Public API request (www)
    async fn www_request(&self, path: &str, params: Option<&HashMap<String, String>>) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let mut url = format!("{WWW_URL}/{path}");
        if let Some(p) = params {
            if !p.is_empty() {
                let query: String = p.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&");
                url = format!("{url}?{query}");
            }
        }

        let response: serde_json::Value = self.www_client.get(&url, None, None).await?;
        self.check_error(&response)?;
        Ok(response)
    }

    /// Public API request (v1)
    async fn v1_public_request(&self, path: &str) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("{V1_URL}/{path}");
        let response: serde_json::Value = self.v1_client.get(&url, None, None).await?;
        self.check_error(&response)?;
        Ok(response)
    }

    /// Private API request (v1 POST)
    async fn v1_private_request(&self, path: &str, params: &HashMap<String, serde_json::Value>) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".to_string(),
        })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        // Build body JSON
        let body = if params.is_empty() {
            "{}".to_string()
        } else {
            serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string())
        };

        // Create auth payload
        let auth = serde_json::json!({
            "timeStamp_nonce": nonce,
            "body": body
        });
        let auth_str = serde_json::to_string(&auth).unwrap();
        let payload = BASE64.encode(auth_str.as_bytes());
        let signature = self.create_signature(&payload)?;

        let mut headers = HashMap::new();
        headers.insert("X-BITBNS-APIKEY".to_string(), api_key.to_string());
        headers.insert("X-BITBNS-PAYLOAD".to_string(), payload);
        headers.insert("X-BITBNS-SIGNATURE".to_string(), signature);
        headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());

        let url = format!("{V1_URL}/{path}");
        let body_value: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let response: serde_json::Value = self.v1_client.post(&url, Some(body_value), Some(headers)).await?;

        self.check_error(&response)?;
        Ok(response)
    }

    /// Private API request (v2 POST)
    async fn v2_private_request(&self, path: &str, params: &HashMap<String, serde_json::Value>) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".to_string(),
        })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        let body = if params.is_empty() {
            "{}".to_string()
        } else {
            serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string())
        };

        let auth = serde_json::json!({
            "timeStamp_nonce": nonce,
            "body": body
        });
        let auth_str = serde_json::to_string(&auth).unwrap();
        let payload = BASE64.encode(auth_str.as_bytes());
        let signature = self.create_signature(&payload)?;

        let mut headers = HashMap::new();
        headers.insert("X-BITBNS-APIKEY".to_string(), api_key.to_string());
        headers.insert("X-BITBNS-PAYLOAD".to_string(), payload);
        headers.insert("X-BITBNS-SIGNATURE".to_string(), signature);
        headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());

        let url = format!("{V2_URL}/{path}");
        let body_value: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let response: serde_json::Value = self.v2_client.post(&url, Some(body_value), Some(headers)).await?;

        self.check_error(&response)?;
        Ok(response)
    }

    /// Check for API errors
    fn check_error(&self, response: &serde_json::Value) -> CcxtResult<()> {
        let code = response.get("code").and_then(|c| c.as_i64());
        let error = response.get("error").and_then(|e| e.as_str());
        let msg = response.get("msg").and_then(|m| m.as_str());

        if let Some(c) = code {
            if c != 200 && c != 204 {
                let err_msg = error.or(msg).unwrap_or("Unknown error");
                return match c {
                    400 => Err(CcxtError::BadRequest { message: err_msg.to_string() }),
                    409 => Err(CcxtError::BadSymbol { symbol: err_msg.to_string() }),
                    416 => Err(CcxtError::InsufficientFunds {
                        currency: "".to_string(),
                        required: "".to_string(),
                        available: err_msg.to_string()
                    }),
                    417 => Err(CcxtError::OrderNotFound { order_id: "unknown".to_string() }),
                    _ => Err(CcxtError::ExchangeError { message: format!("Code {c}: {err_msg}") }),
                };
            }
        }

        if let Some(err) = error {
            if !err.is_empty() && error != Some("null") {
                return Err(CcxtError::ExchangeError { message: err.to_string() });
            }
        }

        Ok(())
    }

    /// Parse order status
    fn parse_order_status(&self, status: i32) -> OrderStatus {
        match status {
            -1 => OrderStatus::Canceled,
            0 | 1 => OrderStatus::Open,
            2 => OrderStatus::Closed,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order
    fn parse_order(&self, order: &serde_json::Value, symbol: &str) -> Order {
        let id = order.get("id")
            .or(order.get("entry_id"))
            .and_then(|i| i.as_i64())
            .map(|i| i.to_string())
            .unwrap_or_default();

        let datetime = order.get("time").and_then(|t| t.as_str());
        let timestamp = datetime.and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|dt| dt.timestamp_millis());

        let side_type = order.get("type").and_then(|t| t.as_i64());
        let side = match side_type {
            Some(0) => OrderSide::Buy,
            Some(1) => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let status_val = order.get("status").and_then(|s| s.as_i64()).unwrap_or(0) as i32;
        let data = order.get("data").and_then(|d| d.as_str());
        let status = if data == Some("Successfully cancelled the order") {
            OrderStatus::Canceled
        } else {
            self.parse_order_status(status_val)
        };

        let price = order.get("rate")
            .and_then(|r| r.as_f64())
            .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));

        let amount = order.get("btc")
            .and_then(|a| a.as_f64())
            .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO))
            .unwrap_or(Decimal::ZERO);

        let trigger_price = order.get("t_rate")
            .and_then(|t| t.as_f64())
            .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));

        Order {
            id,
            client_order_id: None,
            timestamp,
            datetime: datetime.map(|d| d.to_string()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: trigger_price,
            stop_loss_price: None,
            take_profit_price: None,
            trigger_price,
            cost: None,
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            info: order.clone(),
        }
    }
}

#[async_trait]
impl Exchange for Bitbns {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitbns
    }

    fn name(&self) -> &str {
        "Bitbns"
    }

    fn version(&self) -> &str {
        "v2"
    }

    fn countries(&self) -> &[&str] {
        &["IN"]
    }

    fn rate_limit(&self) -> u64 {
        RATE_LIMIT_MS
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
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut final_headers = headers.unwrap_or_default();
        let base_url = match api {
            "www" => WWW_URL,
            "v1" => V1_URL,
            "v2" => V2_URL,
            _ => WWW_URL,
        };

        let mut url = format!("{base_url}/{path}");

        if api != "www" {
            if let Some(api_key) = self.config.api_key() {
                final_headers.insert("X-BITBNS-APIKEY".to_string(), api_key.to_string());
            }
        }

        if method == "GET" && !params.is_empty() {
            let query: String = params.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers: final_headers,
            body: None,
        }
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        {
            let markets = self.markets.read().unwrap();
            if !reload && !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let fetched_markets = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for market in fetched_markets {
            markets_by_id_map.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        {
            let mut markets = self.markets.write().unwrap();
            *markets = markets_map.clone();
        }
        {
            let mut markets_by_id = self.markets_by_id.write().unwrap();
            *markets_by_id = markets_by_id_map;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response = self.www_request("order/fetchMarkets", None).await?;

        let markets_arr: Vec<BitbnsMarket> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "Vec<BitbnsMarket>".to_string(),
                message: e.to_string(),
            })?;

        let mut markets = Vec::new();

        for m in markets_arr {
            let base = m.base.unwrap_or_default();
            let quote = m.quote.unwrap_or("INR".to_string());
            let id = m.id.unwrap_or_default();
            let symbol = format!("{base}/{quote}");

            let precision = m.precision.as_ref();
            let limits = m.limits.as_ref();

            let amount_limits = limits.and_then(|l| l.amount.as_ref());
            let price_limits = limits.and_then(|l| l.price.as_ref());
            let cost_limits = limits.and_then(|l| l.cost.as_ref());

            markets.push(Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.to_lowercase(),
                quote_id: quote.to_lowercase(),
                active: m.active.unwrap_or(true),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                settle: None,
                settle_id: None,
                contract_size: None,
                linear: None,
                inverse: None,
                sub_type: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: precision.and_then(|p| p.amount),
                    price: precision.and_then(|p| p.price),
                    cost: Some(8),
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: amount_limits.and_then(|l| l.min.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))),
                        max: amount_limits.and_then(|l| l.max.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))),
                    },
                    price: MinMax {
                        min: price_limits.and_then(|l| l.min.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))),
                        max: price_limits.and_then(|l| l.max.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))),
                    },
                    cost: MinMax {
                        min: cost_limits.and_then(|l| l.min.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))),
                        max: cost_limits.and_then(|l| l.max.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))),
                    },
                    leverage: MinMax { min: None, max: None },
                },
                margin_modes: None,
                created: None,
                info: serde_json::Value::Null,
                maker: Some(Decimal::from_str("0.0025").unwrap()),
                taker: Some(Decimal::from_str("0.0025").unwrap()),
                percentage: true,
                tier_based: false,
            });
        }

        Ok(markets)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        Err(CcxtError::NotSupported {
            feature: "fetchCurrencies".to_string(),
        })
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        // Emulated via fetchTickers
        let tickers = self.fetch_tickers(Some(&[symbol])).await?;
        tickers.get(symbol).cloned().ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response = self.www_request("order/fetchTickers", None).await?;

        let tickers_obj = response.as_object().ok_or_else(|| CcxtError::ParseError {
            data_type: "tickers".to_string(),
            message: "Expected object response".to_string(),
        })?;

        let mut result = HashMap::new();

        for (symbol_key, ticker_val) in tickers_obj {
            // Filter by requested symbols
            if let Some(syms) = symbols {
                if !syms.contains(&symbol_key.as_str()) {
                    continue;
                }
            }

            let timestamp = ticker_val.get("timestamp").and_then(|t| t.as_i64());
            let high = ticker_val.get("high").and_then(|h| h.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let low = ticker_val.get("low").and_then(|l| l.as_str())
                .and_then(|s| Decimal::from_str(s).ok());
            let bid = ticker_val.get("bid").and_then(|b| b.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let ask = ticker_val.get("ask").and_then(|a| a.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let open = ticker_val.get("open").and_then(|o| o.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let last = ticker_val.get("last").and_then(|l| l.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let base_volume = ticker_val.get("baseVolume").and_then(|v| v.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let change = ticker_val.get("change").and_then(|c| c.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let percentage = ticker_val.get("percentage").and_then(|p| p.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
            let average = ticker_val.get("average").and_then(|a| a.as_f64())
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));

            result.insert(symbol_key.clone(), Ticker {
                symbol: symbol_key.clone(),
                timestamp,
                datetime: timestamp.map(|t| {
                    chrono::DateTime::from_timestamp_millis(t)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                high,
                low,
                bid,
                bid_volume: None,
                ask,
                ask_volume: None,
                vwap: None,
                open,
                close: last,
                last,
                previous_close: None,
                change,
                percentage,
                average,
                base_volume,
                quote_volume: None,
                index_price: None,
                mark_price: None,
                info: ticker_val.clone(),
            });
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response = self.www_request("order/fetchOrderbook", Some(&params)).await?;

        let order_book: BitbnsOrderBook = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "BitbnsOrderBook".to_string(),
                message: e.to_string(),
            })?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(bids_arr) = order_book.bids {
            for bid in bids_arr {
                if bid.len() >= 2 {
                    let price = Decimal::from_f64_retain(bid[0]).unwrap_or(Decimal::ZERO);
                    let amount = Decimal::from_f64_retain(bid[1]).unwrap_or(Decimal::ZERO);
                    bids.push(OrderBookEntry { price, amount });
                }
            }
        }

        if let Some(asks_arr) = order_book.asks {
            for ask in asks_arr {
                if ask.len() >= 2 {
                    let price = Decimal::from_f64_retain(ask[0]).unwrap_or(Decimal::ZERO);
                    let amount = Decimal::from_f64_retain(ask[1]).unwrap_or(Decimal::ZERO);
                    asks.push(OrderBookEntry { price, amount });
                }
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: order_book.timestamp,
            datetime: order_book.timestamp.map(|t| {
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
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let (base_id, quote_id) = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            (market.base.clone(), market.quote.clone())
        };

        let mut params = HashMap::new();
        params.insert("coin".to_string(), base_id);
        params.insert("market".to_string(), quote_id);

        let response = self.www_request("exchangeData/tradedetails", Some(&params)).await?;

        let trades_arr: Vec<BitbnsTrade> = serde_json::from_value(response)
            .map_err(|e| CcxtError::ParseError {
                data_type: "Vec<BitbnsTrade>".to_string(),
                message: e.to_string(),
            })?;

        let mut trades = Vec::new();
        for t in trades_arr {
            let timestamp = t.timestamp;
            let price = t.price.as_ref()
                .and_then(|p| Decimal::from_str(p).ok())
                .unwrap_or(Decimal::ZERO);
            let amount = t.base_volume
                .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO))
                .unwrap_or(Decimal::ZERO);
            let cost = t.quote_volume
                .map(|c| Decimal::from_f64_retain(c).unwrap_or(Decimal::ZERO));

            let side = t.trade_type.as_ref().map(|s| {
                if s.contains("buy") { "buy" } else { "sell" }
            });

            trades.push(Trade {
                id: t.trade_id.unwrap_or_default(),
                order: None,
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: symbol.to_string(),
                trade_type: None,
                side: side.map(|s| s.to_string()),
                taker_or_maker: None,
                price,
                amount,
                cost,
                fee: None,
                fees: vec![],
                info: serde_json::Value::Null,
            });
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
            feature: "fetchOHLCV".to_string(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response = self.v1_private_request("currentCoinBalance/EVERYTHING", &HashMap::new()).await?;

        let mut balances = HashMap::new();

        if let Some(data) = response.get("data").and_then(|d| d.as_object()) {
            for (key, value) in data {
                if key.starts_with("availableorder") {
                    let currency_id = key.strip_prefix("availableorder").unwrap_or("");
                    let currency = if currency_id == "Money" { "INR" } else { currency_id };

                    let free = value.as_f64()
                        .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO));
                    let used_key = format!("inorder{currency_id}");
                    let used = data.get(&used_key)
                        .and_then(|u| u.as_f64())
                        .map(|u| Decimal::from_f64_retain(u).unwrap_or(Decimal::ZERO));
                    let total = free.zip(used).map(|(f, u)| f + u);

                    balances.insert(currency.to_string(), Balance {
                        free,
                        used,
                        total,
                        debt: None,
                    });
                }
            }
        }

        Ok(Balances {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: balances,
            info: response,
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
        let uppercase_id = self.get_uppercase_id(symbol)?;
        let quote_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.quote.clone()
        };

        let mut params = HashMap::new();
        params.insert("side".to_string(), serde_json::Value::String(
            if side == OrderSide::Buy { "BUY" } else { "SELL" }.to_string()
        ));
        params.insert("symbol".to_string(), serde_json::Value::String(uppercase_id));
        params.insert("quantity".to_string(), serde_json::Value::String(amount.to_string()));

        let response = match order_type {
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".to_string(),
                })?;
                params.insert("rate".to_string(), serde_json::Value::String(p.to_string()));
                self.v2_private_request("orders", &params).await?
            }
            OrderType::Market => {
                params.insert("market".to_string(), serde_json::Value::String(quote_id));
                // Use v1 for market orders
                let path = format!("placeMarketOrderQnty/{}", symbol.split('/').next().unwrap_or(""));
                self.v1_private_request(&path, &params).await?
            }
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?}"),
                });
            }
        };

        let order_id = response.get("id")
            .and_then(|i| i.as_i64())
            .map(|i| i.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Ok(Order {
            id: order_id,
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
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            trigger_price: None,
            cost: None,
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            info: response,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let uppercase_id = self.get_uppercase_id(symbol)?;
        let quote_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.quote.clone()
        };

        let quote_side = if quote_id == "USDT" { "usdtcancelOrder" } else { "cancelOrder" };

        let mut params = HashMap::new();
        params.insert("entry_id".to_string(), serde_json::Value::String(id.to_string()));
        params.insert("symbol".to_string(), serde_json::Value::String(uppercase_id));
        params.insert("side".to_string(), serde_json::Value::String(quote_side.to_string()));

        let response = self.v2_private_request("cancel", &params).await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            stop_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            trigger_price: None,
            cost: None,
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            info: response,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("entry_id".to_string(), serde_json::Value::String(id.to_string()));

        let path = format!("orderStatus/{market_id}");
        let response = self.v1_private_request(&path, &params).await?;

        let data = response.get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })?;

        Ok(self.parse_order(data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol required for fetchOpenOrders".to_string(),
        })?;

        let uppercase_id = self.get_uppercase_id(symbol)?;
        let quote_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.quote.clone()
        };

        let quote_side = if quote_id == "USDT" { "usdtListOpenOrders" } else { "listOpenOrders" };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), serde_json::Value::String(uppercase_id));
        params.insert("page".to_string(), serde_json::Value::Number(0.into()));
        params.insert("side".to_string(), serde_json::Value::String(quote_side.to_string()));

        let response = self.v2_private_request("getordersnew", &params).await?;

        let empty_vec = vec![];
        let data = response.get("data")
            .and_then(|d| d.as_array())
            .unwrap_or(&empty_vec);

        let mut orders = Vec::new();
        for order in data {
            orders.push(self.parse_order(order, symbol));
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetchClosedOrders".to_string(),
        })
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Symbol required for fetchMyTrades".to_string(),
        })?;

        let (market_id, quote) = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            (market.id.clone(), market.quote.clone())
        };

        let mut params = HashMap::new();
        params.insert("page".to_string(), serde_json::Value::Number(0.into()));

        if let Some(s) = since {
            let since_str = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
                .unwrap_or_default();
            params.insert("since".to_string(), serde_json::Value::String(since_str));
        }

        let path = format!("listExecutedOrders/{market_id}");
        let response = self.v1_private_request(&path, &params).await?;

        let empty_vec = vec![];
        let data = response.get("data")
            .and_then(|d| d.as_array())
            .unwrap_or(&empty_vec);

        let mut trades = Vec::new();
        for item in data {
            let timestamp = item.get("date").and_then(|d| d.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis());

            let rate = item.get("rate").and_then(|r| r.as_f64())
                .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO))
                .unwrap_or(Decimal::ZERO);

            let factor = item.get("factor").and_then(|f| f.as_f64()).unwrap_or(1.0);
            let crypto = item.get("crypto").and_then(|c| c.as_f64()).unwrap_or(0.0);
            let amount = Decimal::from_f64_retain(crypto / factor).unwrap_or(Decimal::ZERO);

            let trade_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let side = if trade_type.contains("Buy") || trade_type.contains("buy") {
                "buy"
            } else {
                "sell"
            };

            let fee_cost = item.get("fee").and_then(|f| f.as_f64())
                .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO));

            let trade_id = item.get("id").and_then(|i| i.as_str())
                .unwrap_or("");

            trades.push(Trade {
                id: trade_id.to_string(),
                order: Some(trade_id.to_string()),
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(side.to_string()),
                taker_or_maker: None,
                price: rate,
                amount,
                cost: Some(rate * amount),
                fee: fee_cost.map(|cost| Fee {
                    cost: Some(cost),
                    currency: Some(quote.clone()),
                    rate: None,
                }),
                fees: vec![],
                info: item.clone(),
            });
        }

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        Err(CcxtError::NotSupported {
            feature: "fetchDeposits".to_string(),
        })
    }

    async fn fetch_withdrawals(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        Err(CcxtError::NotSupported {
            feature: "fetchWithdrawals".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Bitbns::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Bitbns);
        assert_eq!(exchange.name(), "Bitbns");
        assert_eq!(exchange.countries(), &["IN"]);
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
        assert!(exchange.has().fetch_tickers);
        assert!(exchange.has().fetch_order_book);
        assert!(exchange.has().fetch_trades);
    }

    #[test]
    fn test_uppercase_id() {
        let config = ExchangeConfig::default();
        let exchange = Bitbns::new(config).unwrap();

        // Without loaded markets, parse directly
        let id = exchange.get_uppercase_id("BTC/INR").unwrap();
        assert_eq!(id, "BTC");

        let id_usdt = exchange.get_uppercase_id("BTC/USDT").unwrap();
        assert_eq!(id_usdt, "BTC_USDT");
    }

    #[test]
    fn test_rate_limit() {
        let config = ExchangeConfig::default();
        let exchange = Bitbns::new(config).unwrap();
        assert_eq!(exchange.rate_limit(), 1000);
    }
}
