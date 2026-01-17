//! NovaDAX Exchange Implementation
//!
//! Brazilian cryptocurrency exchange

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::Sha256;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

const API_URL: &str = "https://api.novadax.com";
const RATE_LIMIT_MS: u64 = 10;

/// NovaDAX Exchange
pub struct Novadax {
    config: ExchangeConfig,
    http_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

#[derive(Debug, Deserialize)]
struct NovadaxResponse<T> {
    code: String,
    data: Option<T>,
    message: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct NovadaxMarket {
    symbol: String,
    #[serde(rename = "baseCurrency")]
    base_currency: String,
    #[serde(rename = "quoteCurrency")]
    quote_currency: String,
    #[serde(rename = "amountPrecision")]
    amount_precision: Option<i32>,
    #[serde(rename = "pricePrecision")]
    price_precision: Option<i32>,
    #[serde(rename = "minOrderAmount")]
    min_order_amount: Option<String>,
    #[serde(rename = "minOrderValue")]
    min_order_value: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct NovadaxTicker {
    symbol: String,
    #[serde(rename = "lastPrice")]
    last_price: Option<String>,
    bid: Option<String>,
    ask: Option<String>,
    #[serde(rename = "high24h")]
    high_24h: Option<String>,
    #[serde(rename = "low24h")]
    low_24h: Option<String>,
    #[serde(rename = "open24h")]
    open_24h: Option<String>,
    #[serde(rename = "baseVolume24h")]
    base_volume_24h: Option<String>,
    #[serde(rename = "quoteVolume24h")]
    quote_volume_24h: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct NovadaxOrderBook {
    bids: Option<Vec<Vec<String>>>,
    asks: Option<Vec<Vec<String>>>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct NovadaxTrade {
    amount: Option<String>,
    price: Option<String>,
    side: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct NovadaxBalance {
    currency: String,
    available: Option<String>,
    balance: Option<String>,
    hold: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct NovadaxOrder {
    id: Option<String>,
    symbol: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    side: Option<String>,
    price: Option<String>,
    #[serde(rename = "averagePrice")]
    average_price: Option<String>,
    amount: Option<String>,
    #[serde(rename = "filledAmount")]
    filled_amount: Option<String>,
    value: Option<String>,
    #[serde(rename = "filledValue")]
    filled_value: Option<String>,
    #[serde(rename = "filledFee")]
    filled_fee: Option<String>,
    status: Option<String>,
    timestamp: Option<i64>,
    #[serde(rename = "stopPrice")]
    stop_price: Option<String>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct NovadaxFill {
    id: Option<String>,
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    symbol: Option<String>,
    side: Option<String>,
    amount: Option<String>,
    price: Option<String>,
    fee: Option<String>,
    #[serde(rename = "feeAmount")]
    fee_amount: Option<String>,
    #[serde(rename = "feeCurrency")]
    fee_currency: Option<String>,
    role: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct NovadaxKline {
    #[serde(rename = "openPrice")]
    open_price: Option<f64>,
    #[serde(rename = "highPrice")]
    high_price: Option<f64>,
    #[serde(rename = "lowPrice")]
    low_price: Option<f64>,
    #[serde(rename = "closePrice")]
    close_price: Option<f64>,
    amount: Option<f64>,
    vol: Option<f64>,
    score: Option<i64>,
}

impl Novadax {
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let http_client = HttpClient::new(API_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let features = crate::feature_flags! {
            spot,
            fetch_markets,
            fetch_ticker,
            fetch_tickers,
            fetch_order_book,
            fetch_trades,
            fetch_ohlcv,
            fetch_balance,
            create_order,
            create_limit_order,
            create_market_order,
            cancel_order,
            fetch_order,
            fetch_orders,
            fetch_open_orders,
            fetch_closed_orders,
            fetch_my_trades,
            ws,
            watch_ticker,
            watch_order_book,
            watch_trades,
        };

        let urls = crate::exchange_urls! {
            logo: "https://user-images.githubusercontent.com/1294454/92337550-2b085500-f0b3-11ea-98e7-5794fb07dd3b.jpg",
            www: "https://www.novadax.com.br",
            api: {
                "public" => API_URL,
                "private" => API_URL,
            },
            doc: ["https://doc.novadax.com/pt-BR/"],
            fees: "https://www.novadax.com.br/fees-and-limits",
        };

        let timeframes = crate::timeframe_map! {
            Minute1 => "ONE_MIN",
            Minute5 => "FIVE_MIN",
            Minute15 => "FIFTEEN_MIN",
            Minute30 => "HALF_HOU",
            Hour1 => "ONE_HOU",
            Day1 => "ONE_DAY",
            Week1 => "ONE_WEE",
            Month1 => "ONE_MON",
        };

        Ok(Self {
            config,
            http_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
        })
    }

    /// Convert timeframe to NovaDAX unit
    fn get_timeframe_unit(&self, timeframe: Timeframe) -> &'static str {
        match timeframe {
            Timeframe::Minute1 => "ONE_MIN",
            Timeframe::Minute5 => "FIVE_MIN",
            Timeframe::Minute15 => "FIFTEEN_MIN",
            Timeframe::Minute30 => "HALF_HOU",
            Timeframe::Hour1 => "ONE_HOU",
            Timeframe::Day1 => "ONE_DAY",
            Timeframe::Week1 => "ONE_WEE",
            Timeframe::Month1 => "ONE_MON",
            _ => "ONE_MIN",
        }
    }

    /// Send public GET request
    async fn public_request(
        &self,
        path: &str,
        params: Option<&HashMap<String, String>>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("/v1/{path}");
        let query_params = params.cloned();

        let response: serde_json::Value = self.http_client.get(&url, query_params, None).await?;
        self.handle_response(&response)?;
        Ok(response)
    }

    /// Send private request with authentication
    async fn private_request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, serde_json::Value>>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

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
                message: "Secret required".to_string(),
            })?;

        let timestamp = Utc::now().timestamp_millis().to_string();
        let request_path = format!("/v1/{path}");
        let url = request_path.clone();

        let mut headers = HashMap::new();
        headers.insert("X-Nova-Access-Key".to_string(), api_key.to_string());
        headers.insert("X-Nova-Timestamp".to_string(), timestamp.clone());

        let (query_string, body_value, query_params) = if method == "POST" {
            let body_json = params
                .map(|p| serde_json::to_value(p).unwrap_or_default())
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            let body_str = serde_json::to_string(&body_json).unwrap_or_else(|_| "{}".to_string());
            // MD5 hash of body
            let md5_hash = format!("{:x}", md5::compute(body_str.as_bytes()));
            headers.insert("Content-Type".to_string(), "application/json".to_string());
            (md5_hash, Some(body_json), None)
        } else {
            // URL encode query params (sorted by keys)
            if let Some(p) = params {
                let mut sorted_keys: Vec<&String> = p.keys().collect();
                sorted_keys.sort();
                let query: Vec<String> = sorted_keys
                    .iter()
                    .map(|k| {
                        let v = p.get(*k).unwrap();
                        let val_str = match v {
                            serde_json::Value::String(s) => s.clone(),
                            _ => v.to_string().trim_matches('"').to_string(),
                        };
                        format!("{k}={val_str}")
                    })
                    .collect();
                let query_str = query.join("&");
                // Build query params HashMap for HttpClient
                let mut query_map = HashMap::new();
                for (k, v) in p.iter() {
                    let val_str = match v {
                        serde_json::Value::String(s) => s.clone(),
                        _ => v.to_string().trim_matches('"').to_string(),
                    };
                    query_map.insert(k.clone(), val_str);
                }
                (query_str, None, Some(query_map))
            } else {
                (String::new(), None, None)
            }
        };

        // Auth: METHOD\n/path\nqueryString\ntimestamp
        let auth = format!("{method}\n{request_path}\n{query_string}\n{timestamp}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            }
        })?;
        mac.update(auth.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        headers.insert("X-Nova-Signature".to_string(), signature);

        let response: serde_json::Value = if method == "POST" {
            self.http_client
                .post(&url, body_value, Some(headers))
                .await?
        } else {
            self.http_client
                .get(&url, query_params, Some(headers))
                .await?
        };

        self.handle_response(&response)?;
        Ok(response)
    }

    /// Handle API response and check for errors
    fn handle_response(&self, response: &serde_json::Value) -> CcxtResult<()> {
        let code = response.get("code").and_then(|c| c.as_str()).unwrap_or("");

        if code != "A10000" {
            let message = response
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return match code {
                "A10001" => Err(CcxtError::BadRequest {
                    message: message.to_string(),
                }),
                "A10003" => Err(CcxtError::AuthenticationError {
                    message: message.to_string(),
                }),
                "A10004" => Err(CcxtError::RateLimitExceeded {
                    message: message.to_string(),
                    retry_after_ms: None,
                }),
                "A10011" | "A10012" => Err(CcxtError::BadSymbol {
                    symbol: message.to_string(),
                }),
                "A10013" => Err(CcxtError::OnMaintenance {
                    message: message.to_string(),
                }),
                "A30001" => Err(CcxtError::OrderNotFound {
                    order_id: "unknown".to_string(),
                }),
                "A30007" | "A40004" => Err(CcxtError::InsufficientFunds {
                    currency: "".to_string(),
                    required: "".to_string(),
                    available: message.to_string(),
                }),
                _ => Err(CcxtError::ExchangeError {
                    message: format!("{code}: {message}"),
                }),
            };
        }

        Ok(())
    }

    /// Parse order status
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "SUBMITTED" | "PROCESSING" | "PARTIAL_FILLED" | "CANCELING" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order from API response
    fn parse_order(&self, order: &NovadaxOrder, symbol: &str) -> Order {
        let timestamp = order.timestamp;
        let datetime = timestamp.map(|ts| {
            chrono::DateTime::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        let price = order.price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let average = order
            .average_price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let amount = order
            .amount
            .as_ref()
            .and_then(|a| Decimal::from_str(a).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = order
            .filled_amount
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok())
            .unwrap_or(Decimal::ZERO);
        let cost = order
            .filled_value
            .as_ref()
            .and_then(|c| Decimal::from_str(c).ok());

        let status_str = order.status.as_deref().unwrap_or("");
        let status = self.parse_order_status(status_str);

        let side = match order.side.as_deref() {
            Some("BUY") | Some("buy") => OrderSide::Buy,
            _ => OrderSide::Sell,
        };

        let order_type = match order.order_type.as_deref() {
            Some("LIMIT") | Some("STOP_LIMIT") => OrderType::Limit,
            Some("MARKET") | Some("STOP_MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let fee_cost = order
            .filled_fee
            .as_ref()
            .and_then(|f| Decimal::from_str(f).ok());

        Order {
            id: order.id.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp,
            datetime,
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
            remaining: Some(amount - filled),
            stop_price: None,
            cost,
            trigger_price: order
                .stop_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok()),
            stop_loss_price: None,
            take_profit_price: None,
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: fee_cost.map(|c| Fee {
                cost: Some(c),
                currency: None,
                rate: None,
            }),
            fees: vec![],
            info: serde_json::to_value(order).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Novadax {
    fn id(&self) -> ExchangeId {
        ExchangeId::Novadax
    }

    fn name(&self) -> &str {
        "NovaDAX"
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
            let mut cached_markets = self.markets.write().unwrap();
            *cached_markets = markets_map.clone();
        }
        {
            let mut cached_markets_by_id = self.markets_by_id.write().unwrap();
            *cached_markets_by_id = markets_by_id;
        }

        Ok(markets_map)
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
        body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{API_URL}/v1/{path}");
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();
            let timestamp = Utc::now().timestamp_millis().to_string();
            let request_path = format!("/v1/{path}");

            headers.insert("X-Nova-Access-Key".into(), api_key.to_string());
            headers.insert("X-Nova-Timestamp".into(), timestamp.clone());

            let (query_string, body_string) = if method == "POST" {
                let body_content = body.unwrap_or("{}");
                let md5_hash = format!("{:x}", md5::compute(body_content.as_bytes()));
                headers.insert("Content-Type".into(), "application/json".into());
                (md5_hash, Some(body_content.to_string()))
            } else {
                let mut sorted_keys: Vec<&String> = params.keys().collect();
                sorted_keys.sort();
                let query: Vec<String> = sorted_keys
                    .iter()
                    .map(|k| format!("{}={}", k, params.get(*k).unwrap()))
                    .collect();
                let query_str = query.join("&");
                if !query_str.is_empty() {
                    url = format!("{url}?{query_str}");
                }
                (query_str, None)
            };

            let auth = format!("{method}\n{request_path}\n{query_string}\n{timestamp}");

            if let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) {
                mac.update(auth.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());
                headers.insert("X-Nova-Signature".into(), signature);
            }

            SignedRequest {
                url,
                method: method.to_string(),
                headers,
                body: body_string,
            }
        } else {
            if !params.is_empty() {
                let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
                url = format!("{}?{}", url, query.join("&"));
            }

            SignedRequest {
                url,
                method: method.to_string(),
                headers,
                body: None,
            }
        }
    }

    async fn fetch_time(&self) -> CcxtResult<i64> {
        let response = self.public_request("common/timestamp", None).await?;
        let timestamp = response
            .get("data")
            .and_then(|d| d.as_i64())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "timestamp".to_string(),
                message: "Failed to parse timestamp".to_string(),
            })?;
        Ok(timestamp)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response = self.public_request("common/symbols", None).await?;
        let data: Vec<NovadaxMarket> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let mut markets = Vec::new();
        for m in data {
            let base = m.base_currency.clone();
            let quote = m.quote_currency.clone();
            let symbol = format!("{base}/{quote}");
            let active = m.status.as_deref() == Some("ONLINE");

            let amount_precision = m.amount_precision.unwrap_or(8);
            let price_precision = m.price_precision.unwrap_or(8);

            let min_amount = m
                .min_order_amount
                .as_ref()
                .and_then(|a| Decimal::from_str(a).ok());
            let min_cost = m
                .min_order_value
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok());

            let info = serde_json::to_value(&m).unwrap_or_default();
            let market = Market {
                id: m.symbol.clone(),
                lowercase_id: Some(m.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: m.base_currency.clone(),
                quote_id: m.quote_currency.clone(),
                active,
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
            underlying: None,
            underlying_id: None,
                margin_modes: None,
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: min_amount,
                        max: None,
                    },
                    price: MinMax {
                        min: None,
                        max: None,
                    },
                    cost: MinMax {
                        min: min_cost,
                        max: None,
                    },
                    leverage: MinMax {
                        min: None,
                        max: None,
                    },
                },
                created: None,
                info,
                taker: Some(Decimal::from_str("0.005").unwrap()),
                maker: Some(Decimal::from_str("0.0025").unwrap()),
                percentage: true,
                tier_based: false,
            };

            markets.push(market);
        }

        // Update markets cache
        {
            let mut markets_cache = self.markets.write().unwrap();
            for m in &markets {
                markets_cache.insert(m.symbol.clone(), m.clone());
            }
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response = self.public_request("market/ticker", Some(&params)).await?;
        let ticker: NovadaxTicker = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "NovadaxTicker".to_string(),
                message: "Failed to parse ticker".to_string(),
            })?;

        let timestamp = ticker.timestamp;
        let datetime = timestamp.map(|ts| {
            chrono::DateTime::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        let last = ticker
            .last_price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let open = ticker
            .open_24h
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let high = ticker
            .high_24h
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let low = ticker
            .low_24h
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let bid = ticker.bid.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let ask = ticker.ask.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let base_volume = ticker
            .base_volume_24h
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let quote_volume = ticker
            .quote_volume_24h
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());

        let change = match (&last, &open) {
            (Some(l), Some(o)) if *o != Decimal::ZERO => Some(*l - *o),
            _ => None,
        };
        let percentage = match (&change, &open) {
            (Some(c), Some(o)) if *o != Decimal::ZERO => Some(*c / *o * Decimal::from(100)),
            _ => None,
        };

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime,
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
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&ticker).unwrap_or_default(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response = self.public_request("market/tickers", None).await?;
        let tickers: Vec<NovadaxTicker> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let mut result = HashMap::new();

        for ticker in tickers {
            let market_id = ticker.symbol.clone();
            // Convert market_id to symbol (BTC_BRL -> BTC/BRL)
            let parts: Vec<&str> = market_id.split('_').collect();
            let symbol = if parts.len() == 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                market_id.clone()
            };

            // Filter by symbols if provided
            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let timestamp = ticker.timestamp;
            let datetime = timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            });

            let last = ticker
                .last_price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok());
            let open = ticker
                .open_24h
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok());
            let high = ticker
                .high_24h
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok());
            let low = ticker
                .low_24h
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok());
            let bid = ticker.bid.as_ref().and_then(|p| Decimal::from_str(p).ok());
            let ask = ticker.ask.as_ref().and_then(|p| Decimal::from_str(p).ok());
            let base_volume = ticker
                .base_volume_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok());
            let quote_volume = ticker
                .quote_volume_24h
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok());

            let change = match (&last, &open) {
                (Some(l), Some(o)) if *o != Decimal::ZERO => Some(*l - *o),
                _ => None,
            };
            let percentage = match (&change, &open) {
                (Some(c), Some(o)) if *o != Decimal::ZERO => Some(*c / *o * Decimal::from(100)),
                _ => None,
            };

            result.insert(
                symbol.clone(),
                Ticker {
                    symbol: symbol.clone(),
                    timestamp,
                    datetime,
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
                    average: None,
                    base_volume,
                    quote_volume,
                    index_price: None,
                    mark_price: None,
                    info: serde_json::to_value(&ticker).unwrap_or_default(),
                },
            );
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

        let response = self.public_request("market/depth", Some(&params)).await?;
        let order_book: NovadaxOrderBook = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "NovadaxOrderBook".to_string(),
                message: "Failed to parse order book".to_string(),
            })?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        if let Some(bids_arr) = order_book.bids {
            for bid in bids_arr {
                if bid.len() >= 2 {
                    let price = Decimal::from_str(&bid[0]).unwrap_or(Decimal::ZERO);
                    let amount = Decimal::from_str(&bid[1]).unwrap_or(Decimal::ZERO);
                    bids.push(OrderBookEntry { price, amount });
                }
            }
        }

        if let Some(asks_arr) = order_book.asks {
            for ask in asks_arr {
                if ask.len() >= 2 {
                    let price = Decimal::from_str(&ask[0]).unwrap_or(Decimal::ZERO);
                    let amount = Decimal::from_str(&ask[1]).unwrap_or(Decimal::ZERO);
                    asks.push(OrderBookEntry { price, amount });
                }
            }
        }

        let timestamp = order_book.timestamp;
        let datetime = timestamp.map(|ts| {
            chrono::DateTime::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        });

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp,
            datetime,
            nonce: None,
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

        let response = self.public_request("market/trades", Some(&params)).await?;
        let trades_arr: Vec<NovadaxTrade> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let mut trades = Vec::new();
        for t in trades_arr {
            let timestamp = t.timestamp;
            let datetime = timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            });

            let price = t
                .price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok())
                .unwrap_or(Decimal::ZERO);
            let amount = t
                .amount
                .as_ref()
                .and_then(|a| Decimal::from_str(a).ok())
                .unwrap_or(Decimal::ZERO);
            let cost = price * amount;

            let side = match t.side.as_deref() {
                Some("BUY") | Some("buy") => "buy",
                _ => "sell",
            };

            let trade_id = format!("{}", timestamp.unwrap_or(0));
            trades.push(Trade {
                id: trade_id,
                order: None,
                timestamp,
                datetime,
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(side.to_string()),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(cost),
                fee: None,
                fees: vec![],
                info: serde_json::to_value(&t).unwrap_or_default(),
            });
        }

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
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let unit = self.get_timeframe_unit(timeframe);
        let limit_val = limit.unwrap_or(3000) as i64;
        let duration = timeframe.to_millis() / 1000;
        let now = Utc::now().timestamp();

        let (from, to) = if let Some(s) = since {
            let start = s / 1000;
            (start, start + limit_val * duration)
        } else {
            (now - limit_val * duration, now)
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("unit".to_string(), unit.to_string());
        params.insert("from".to_string(), from.to_string());
        params.insert("to".to_string(), to.to_string());

        let response = self
            .public_request("market/kline/history", Some(&params))
            .await?;
        let klines: Vec<NovadaxKline> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let mut result = Vec::new();
        for k in klines {
            let timestamp = k.score.map(|s| s * 1000);
            let open = k
                .open_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));
            let high = k
                .high_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));
            let low = k
                .low_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));
            let close = k
                .close_price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));
            let volume = k
                .amount
                .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));

            result.push(OHLCV {
                timestamp: timestamp.unwrap_or(0),
                open: open.unwrap_or(Decimal::ZERO),
                high: high.unwrap_or(Decimal::ZERO),
                low: low.unwrap_or(Decimal::ZERO),
                close: close.unwrap_or(Decimal::ZERO),
                volume: volume.unwrap_or(Decimal::ZERO),
            });
        }

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response = self
            .private_request("GET", "account/getBalance", None)
            .await?;
        let data: Vec<NovadaxBalance> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let mut balances = HashMap::new();
        for b in data {
            let currency = b.currency.clone();
            let free = b.available.as_ref().and_then(|a| Decimal::from_str(a).ok());
            let total = b.balance.as_ref().and_then(|t| Decimal::from_str(t).ok());
            let used = b.hold.as_ref().and_then(|h| Decimal::from_str(h).ok());

            balances.insert(
                currency,
                Balance {
                    free,
                    used,
                    total,
                    debt: None,
                },
            );
        }

        Ok(Balances {
            timestamp: None,
            datetime: None,
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
        let market_id = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            market.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), serde_json::Value::String(market_id));
        params.insert(
            "side".to_string(),
            serde_json::Value::String(
                if side == OrderSide::Buy {
                    "BUY"
                } else {
                    "SELL"
                }
                .to_string(),
            ),
        );

        let type_str = match order_type {
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".to_string(),
                })?;
                params.insert(
                    "price".to_string(),
                    serde_json::Value::String(p.to_string()),
                );
                params.insert(
                    "amount".to_string(),
                    serde_json::Value::String(amount.to_string()),
                );
                "LIMIT"
            },
            OrderType::Market => {
                if side == OrderSide::Sell {
                    params.insert(
                        "amount".to_string(),
                        serde_json::Value::String(amount.to_string()),
                    );
                } else {
                    // For market buy, need to provide value (quote amount)
                    if let Some(p) = price {
                        let value = amount * p;
                        params.insert(
                            "value".to_string(),
                            serde_json::Value::String(value.to_string()),
                        );
                    } else {
                        // Use amount as value if no price provided
                        params.insert(
                            "value".to_string(),
                            serde_json::Value::String(amount.to_string()),
                        );
                    }
                }
                "MARKET"
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?}"),
                });
            },
        };
        params.insert(
            "type".to_string(),
            serde_json::Value::String(type_str.to_string()),
        );

        let response = self
            .private_request("POST", "orders/create", Some(&params))
            .await?;
        let order: NovadaxOrder = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .ok_or_else(|| CcxtError::ParseError {
                data_type: "NovadaxOrder".to_string(),
                message: "Failed to parse order".to_string(),
            })?;

        Ok(self.parse_order(&order, symbol))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), serde_json::Value::String(id.to_string()));

        let response = self
            .private_request("POST", "orders/cancel", Some(&params))
            .await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: _symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: Some(Decimal::ZERO),
            stop_price: None,
            cost: None,
            trigger_price: None,
            stop_loss_price: None,
            take_profit_price: None,
            trades: vec![],
            fees: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            info: response,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), serde_json::Value::String(id.to_string()));

        let response = self
            .private_request("GET", "orders/get", Some(&params))
            .await?;
        let order: NovadaxOrder = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })?;

        // Get symbol from order response
        let symbol = order
            .symbol
            .as_ref()
            .map(|s| {
                let parts: Vec<&str> = s.split('_').collect();
                if parts.len() == 2 {
                    format!("{}/{}", parts[0], parts[1])
                } else {
                    s.clone()
                }
            })
            .unwrap_or_else(|| _symbol.to_string());

        Ok(self.parse_order(&order, &symbol))
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
                let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                    symbol: s.to_string(),
                })?;
                market.id.clone()
            };
            params.insert("symbol".to_string(), serde_json::Value::String(market_id));
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), serde_json::Value::Number(l.into()));
        }

        if let Some(s) = since {
            params.insert(
                "fromTimestamp".to_string(),
                serde_json::Value::Number(s.into()),
            );
        }

        let response = self
            .private_request("GET", "orders/list", Some(&params))
            .await?;
        let orders: Vec<NovadaxOrder> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let result: Vec<Order> = orders
            .iter()
            .map(|o| {
                let order_symbol = o
                    .symbol
                    .as_ref()
                    .map(|s| {
                        let parts: Vec<&str> = s.split('_').collect();
                        if parts.len() == 2 {
                            format!("{}/{}", parts[0], parts[1])
                        } else {
                            s.clone()
                        }
                    })
                    .unwrap_or_default();
                self.parse_order(o, &order_symbol)
            })
            .collect();

        Ok(result)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert(
            "status".to_string(),
            serde_json::Value::String("SUBMITTED,PROCESSING,PARTIAL_FILLED,CANCELING".to_string()),
        );

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                    symbol: s.to_string(),
                })?;
                market.id.clone()
            };
            params.insert("symbol".to_string(), serde_json::Value::String(market_id));
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), serde_json::Value::Number(l.into()));
        }

        if let Some(s) = since {
            params.insert(
                "fromTimestamp".to_string(),
                serde_json::Value::Number(s.into()),
            );
        }

        let response = self
            .private_request("GET", "orders/list", Some(&params))
            .await?;
        let orders: Vec<NovadaxOrder> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let result: Vec<Order> = orders
            .iter()
            .map(|o| {
                let order_symbol = o
                    .symbol
                    .as_ref()
                    .map(|s| {
                        let parts: Vec<&str> = s.split('_').collect();
                        if parts.len() == 2 {
                            format!("{}/{}", parts[0], parts[1])
                        } else {
                            s.clone()
                        }
                    })
                    .unwrap_or_default();
                self.parse_order(o, &order_symbol)
            })
            .collect();

        Ok(result)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert(
            "status".to_string(),
            serde_json::Value::String("FILLED,CANCELED,REJECTED".to_string()),
        );

        if let Some(s) = symbol {
            let market_id = {
                let markets = self.markets.read().unwrap();
                let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                    symbol: s.to_string(),
                })?;
                market.id.clone()
            };
            params.insert("symbol".to_string(), serde_json::Value::String(market_id));
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), serde_json::Value::Number(l.into()));
        }

        if let Some(s) = since {
            params.insert(
                "fromTimestamp".to_string(),
                serde_json::Value::Number(s.into()),
            );
        }

        let response = self
            .private_request("GET", "orders/list", Some(&params))
            .await?;
        let orders: Vec<NovadaxOrder> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let result: Vec<Order> = orders
            .iter()
            .map(|o| {
                let order_symbol = o
                    .symbol
                    .as_ref()
                    .map(|s| {
                        let parts: Vec<&str> = s.split('_').collect();
                        if parts.len() == 2 {
                            format!("{}/{}", parts[0], parts[1])
                        } else {
                            s.clone()
                        }
                    })
                    .unwrap_or_default();
                self.parse_order(o, &order_symbol)
            })
            .collect();

        Ok(result)
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
                let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                    symbol: s.to_string(),
                })?;
                market.id.clone()
            };
            params.insert("symbol".to_string(), serde_json::Value::String(market_id));
        }

        if let Some(l) = limit {
            params.insert("limit".to_string(), serde_json::Value::Number(l.into()));
        }

        if let Some(s) = since {
            params.insert(
                "fromTimestamp".to_string(),
                serde_json::Value::Number(s.into()),
            );
        }

        let response = self
            .private_request("GET", "orders/fills", Some(&params))
            .await?;
        let fills: Vec<NovadaxFill> = response
            .get("data")
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_default();

        let mut trades = Vec::new();
        for f in fills {
            let fill_symbol = f
                .symbol
                .as_ref()
                .map(|s| {
                    let parts: Vec<&str> = s.split('_').collect();
                    if parts.len() == 2 {
                        format!("{}/{}", parts[0], parts[1])
                    } else {
                        s.clone()
                    }
                })
                .unwrap_or_else(|| symbol.unwrap_or("").to_string());

            let timestamp = f.timestamp;
            let datetime = timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            });

            let price = f
                .price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok())
                .unwrap_or(Decimal::ZERO);
            let amount = f
                .amount
                .as_ref()
                .and_then(|a| Decimal::from_str(a).ok())
                .unwrap_or(Decimal::ZERO);
            let cost = price * amount;

            let side = match f.side.as_deref() {
                Some("BUY") | Some("buy") => "buy",
                _ => "sell",
            };

            let taker_or_maker = f.role.as_ref().map(|r| r.to_lowercase());

            let fee = f
                .fee_amount
                .as_ref()
                .and_then(|fa| Decimal::from_str(fa).ok())
                .map(|c| Fee {
                    cost: Some(c),
                    currency: f.fee_currency.clone(),
                    rate: None,
                });

            let taker_or_maker_enum = taker_or_maker.and_then(|r| match r.as_str() {
                "taker" => Some(TakerOrMaker::Taker),
                "maker" => Some(TakerOrMaker::Maker),
                _ => None,
            });

            let trade_id =
                f.id.clone()
                    .unwrap_or_else(|| format!("{}", timestamp.unwrap_or(0)));
            trades.push(Trade {
                id: trade_id,
                order: f.order_id.clone(),
                timestamp,
                datetime,
                symbol: fill_symbol,
                trade_type: None,
                side: Some(side.to_string()),
                taker_or_maker: taker_or_maker_enum,
                price,
                amount,
                cost: Some(cost),
                fee,
                fees: vec![],
                info: serde_json::to_value(&f).unwrap_or_default(),
            });
        }

        Ok(trades)
    }
}
