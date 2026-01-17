//! Bit2C Exchange Implementation
//!
//! Israeli cryptocurrency exchange (<https://bit2c.co.il>)

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};
use crate::{exchange_urls, feature_flags};

const BASE_URL: &str = "https://bit2c.co.il";
const RATE_LIMIT_MS: u64 = 3000;

/// Bit2C Exchange
pub struct Bit2c {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// API Response Types
#[derive(Debug, Deserialize)]
struct Bit2cTicker {
    #[serde(rename = "ll")]
    last: Option<f64>,
    #[serde(rename = "av")]
    average: Option<f64>,
    #[serde(rename = "a")]
    base_volume: Option<f64>,
    #[serde(rename = "h")]
    bid: Option<f64>,
    #[serde(rename = "l")]
    ask: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Bit2cOrderBook {
    bids: Option<Vec<Vec<f64>>>,
    asks: Option<Vec<Vec<f64>>>,
}

#[derive(Debug, Deserialize)]
struct Bit2cTrade {
    date: Option<i64>,
    price: Option<f64>,
    amount: Option<f64>,
    #[serde(rename = "isBid")]
    is_bid: Option<bool>,
    tid: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Bit2cNewOrder {
    created: Option<i64>,
    #[serde(rename = "type")]
    order_type_num: Option<i32>,
    order_type: Option<i32>,
    status_type: Option<i32>,
    amount: Option<f64>,
    price: Option<f64>,
    id: Option<i64>,
    #[serde(rename = "initialAmount")]
    initial_amount: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Bit2cOrderResponse {
    #[serde(rename = "OrderResponse")]
    order_response: Option<Bit2cOrderMeta>,
    #[serde(rename = "NewOrder")]
    new_order: Option<Bit2cNewOrder>,
}

#[derive(Debug, Deserialize)]
struct Bit2cOrderMeta {
    pair: Option<String>,
    #[serde(rename = "HasError")]
    has_error: Option<bool>,
    #[serde(rename = "Error")]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Bit2cFetchOrder {
    pair: Option<String>,
    status: Option<String>,
    created: Option<i64>,
    #[serde(rename = "type")]
    side_type: Option<i32>,
    order_type: Option<i32>,
    amount: Option<f64>,
    price: Option<f64>,
    id: Option<i64>,
    #[serde(rename = "initialAmount")]
    initial_amount: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Bit2cMyOrdersResponse {
    #[serde(flatten)]
    pairs: HashMap<String, Bit2cMyOrders>,
}

#[derive(Debug, Deserialize)]
struct Bit2cMyOrders {
    ask: Option<Vec<Bit2cFetchOrder>>,
    bid: Option<Vec<Bit2cFetchOrder>>,
}

impl Bit2c {
    /// Create new Bit2C instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(RATE_LIMIT_MS);

        let urls = exchange_urls! {
            logo: "https://github.com/user-attachments/assets/db0bce50-6842-4c09-a1d5-0c87d22118aa",
            www: "https://www.bit2c.co.il",
            api: {
                "rest" => BASE_URL,
            },
            doc: [
                "https://www.bit2c.co.il/home/api",
                "https://github.com/OferE/bit2c",
            ],
        };

        let features = feature_flags! {
            spot,
            fetch_markets,
            fetch_currencies,
            fetch_ticker,
            fetch_order_book,
            fetch_trades,
            fetch_balance,
            create_order,
            create_limit_order,
            create_market_order,
            cancel_order,
            fetch_order,
            fetch_open_orders,
            fetch_my_trades,
            fetch_deposit_address,
            ws,
            watch_ticker,
            watch_order_book,
            watch_trades,
        };

        // No OHLCV support
        let timeframes = HashMap::new();

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

    /// Get hardcoded markets
    fn get_hardcoded_markets(&self) -> Vec<Market> {
        let pairs = [
            ("BtcNis", "BTC", "NIS"),
            ("EthNis", "ETH", "NIS"),
            ("LtcNis", "LTC", "NIS"),
            ("UsdcNis", "USDC", "NIS"),
        ];

        pairs
            .iter()
            .map(|(id, base, quote)| {
                let symbol = format!("{base}/{quote}");
                Market {
                    id: id.to_string(),
                    lowercase_id: Some(id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.to_string(),
                    quote: quote.to_string(),
                    base_id: base.to_lowercase(),
                    quote_id: quote.to_lowercase(),
                    active: true,
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
                    precision: MarketPrecision {
                        amount: Some(8),
                        price: Some(2),
                        cost: Some(8),
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits {
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
                        leverage: MinMax {
                            min: None,
                            max: None,
                        },
                    },
                    margin_modes: None,
                    created: None,
                    info: serde_json::Value::Null,
                    maker: Some(Decimal::from_str("0.025").unwrap()),
                    taker: Some(Decimal::from_str("0.03").unwrap()),
                    percentage: true,
                    tier_based: true,
                }
            })
            .collect()
    }

    /// Get market ID from symbol
    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            Ok(market.id.clone())
        } else {
            // Try to parse symbol directly
            let parts: Vec<&str> = symbol.split('/').collect();
            if parts.len() == 2 {
                // Convert BTC/NIS -> BtcNis
                let base = parts[0];
                let quote = parts[1];
                let base_cap = base.chars().next().unwrap().to_uppercase().to_string()
                    + &base[1..].to_lowercase();
                let quote_cap = quote.chars().next().unwrap().to_uppercase().to_string()
                    + &quote[1..].to_lowercase();
                Ok(format!("{base_cap}{quote_cap}"))
            } else {
                Err(CcxtError::BadSymbol {
                    symbol: symbol.to_string(),
                })
            }
        }
    }

    /// HMAC-SHA512 signature (Base64 encoded)
    fn create_signature(&self, data: &str) -> CcxtResult<String> {
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".to_string(),
            })?;

        type HmacSha512 = Hmac<Sha512>;
        let mut mac = HmacSha512::new_from_slice(secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".to_string(),
            }
        })?;
        mac.update(data.as_bytes());

        Ok(BASE64.encode(mac.finalize().into_bytes()))
    }

    /// Public API request
    async fn public_request(&self, path: &str) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("{BASE_URL}/{path}.json");
        let response: serde_json::Value = self.client.get(&url, None, None).await?;

        // Check for errors
        if let Some(error) = response.get("error").or(response.get("Error")) {
            if let Some(err_msg) = error.as_str() {
                if err_msg.contains("Please provide valid nonce") {
                    return Err(CcxtError::InvalidNonce {
                        message: err_msg.to_string(),
                    });
                }
                if err_msg.contains("Please provide valid APIkey") {
                    return Err(CcxtError::AuthenticationError {
                        message: err_msg.to_string(),
                    });
                }
                if err_msg.contains("No order found") {
                    return Err(CcxtError::OrderNotFound {
                        order_id: "unknown".to_string(),
                    });
                }
                return Err(CcxtError::ExchangeError {
                    message: err_msg.to_string(),
                });
            }
        }

        Ok(response)
    }

    /// Private API request (GET)
    async fn private_get(
        &self,
        path: &str,
        params: &HashMap<String, String>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;

        let nonce = Utc::now().timestamp_millis();
        let mut query_params = params.clone();
        query_params.insert("nonce".to_string(), nonce.to_string());

        // Build query string
        let query: String = query_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let signature = self.create_signature(&query)?;

        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        headers.insert("key".to_string(), api_key.to_string());
        headers.insert("sign".to_string(), signature);

        let url = format!("{BASE_URL}/{path}?{query}");
        let response: serde_json::Value = self.client.get(&url, Some(headers), None).await?;

        self.check_error(&response)?;
        Ok(response)
    }

    /// Private API request (POST)
    async fn private_post(
        &self,
        path: &str,
        params: &HashMap<String, String>,
    ) -> CcxtResult<serde_json::Value> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".to_string(),
            })?;

        let nonce = Utc::now().timestamp_millis();
        let mut form_params = params.clone();
        form_params.insert("nonce".to_string(), nonce.to_string());

        // Build form body
        let body: String = form_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let signature = self.create_signature(&body)?;

        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        headers.insert("key".to_string(), api_key.to_string());
        headers.insert("sign".to_string(), signature);

        let url = format!("{BASE_URL}/{path}");

        // Post with form-encoded body
        let body_value = serde_json::Value::String(body);
        let response: serde_json::Value = self
            .client
            .post(&url, Some(body_value), Some(headers))
            .await?;

        self.check_error(&response)?;
        Ok(response)
    }

    /// Check for API errors
    fn check_error(&self, response: &serde_json::Value) -> CcxtResult<()> {
        if let Some(error) = response.get("error").or(response.get("Error")) {
            if let Some(err_msg) = error.as_str() {
                if err_msg.contains("Please provide valid nonce") {
                    return Err(CcxtError::InvalidNonce {
                        message: err_msg.to_string(),
                    });
                }
                if err_msg.contains("Please provide valid APIkey") {
                    return Err(CcxtError::AuthenticationError {
                        message: err_msg.to_string(),
                    });
                }
                if err_msg.contains("No order found") {
                    return Err(CcxtError::OrderNotFound {
                        order_id: "unknown".to_string(),
                    });
                }
                if err_msg.contains("please approve new terms") {
                    return Err(CcxtError::PermissionDenied {
                        message: err_msg.to_string(),
                    });
                }
                return Err(CcxtError::ExchangeError {
                    message: err_msg.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Parse order from API response
    fn parse_order(&self, order: &Bit2cFetchOrder, symbol: &str) -> Order {
        let status = match order.status.as_deref() {
            Some("New") | Some("Open") => OrderStatus::Open,
            Some("Completed") => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let order_type = match order.order_type {
            Some(0) => OrderType::Limit,
            Some(1) => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match order.side_type {
            Some(0) => OrderSide::Buy,
            Some(1) => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price = order
            .price
            .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));
        let amount = order
            .initial_amount
            .or(order.amount)
            .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO))
            .unwrap_or(Decimal::ZERO);
        let remaining = order
            .amount
            .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO));
        let filled = remaining.map(|r| amount - r);

        Order {
            id: order.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: None,
            timestamp: order.created.map(|t| t * 1000),
            datetime: order.created.map(|t| {
                chrono::DateTime::from_timestamp(t, 0)
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
            filled: filled.unwrap_or(Decimal::ZERO),
            remaining,
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
            info: serde_json::Value::Null,
        }
    }

    /// Parse new order from create response
    fn parse_new_order(
        &self,
        order: &Bit2cNewOrder,
        symbol: &str,
        side: OrderSide,
        order_type: OrderType,
    ) -> Order {
        let status = match order.status_type {
            Some(0) | Some(1) => OrderStatus::Open,
            Some(5) => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let price = order
            .price
            .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO));
        let amount = order
            .amount
            .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO))
            .unwrap_or(Decimal::ZERO);

        Order {
            id: order.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: None,
            timestamp: order.created.map(|t| t * 1000),
            datetime: order.created.map(|t| {
                chrono::DateTime::from_timestamp(t, 0)
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
            info: serde_json::Value::Null,
        }
    }
}

#[async_trait]
impl Exchange for Bit2c {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bit2c
    }

    fn name(&self) -> &str {
        "Bit2C"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["IL"]
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
        self.get_market_id(symbol).ok()
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
        let mut url = format!("{BASE_URL}/{path}");

        if api == "public" {
            url = format!("{url}.json");
        } else if let Some(api_key) = self.config.api_key() {
            let nonce = Utc::now().timestamp_millis();
            let mut query_params = params.clone();
            query_params.insert("nonce".to_string(), nonce.to_string());

            let query: String = query_params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");

            if let Ok(signature) = self.create_signature(&query) {
                final_headers.insert(
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded".to_string(),
                );
                final_headers.insert("key".to_string(), api_key.to_string());
                final_headers.insert("sign".to_string(), signature);
            }

            if method == "GET" && !query_params.is_empty() {
                url = format!("{url}?{query}");
            }
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
        // Bit2C has hardcoded markets
        Ok(self.get_hardcoded_markets())
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        let mut currencies = HashMap::new();

        // Hardcoded currencies based on available markets
        let currency_list = [
            ("BTC", "Bitcoin"),
            ("ETH", "Ethereum"),
            ("LTC", "Litecoin"),
            ("USDC", "USD Coin"),
            ("NIS", "Israeli Shekel"),
        ];

        for (code, name) in currency_list {
            currencies.insert(
                code.to_string(),
                crate::types::Currency {
                    id: code.to_lowercase(),
                    code: code.to_string(),
                    name: Some(name.to_string()),
                    active: true,
                    deposit: Some(code != "NIS"),
                    withdraw: Some(code != "NIS"),
                    fee: None,
                    precision: Some(8),
                    limits: Some(crate::types::CurrencyLimits {
                        withdraw: MinMax {
                            min: None,
                            max: None,
                        },
                        deposit: MinMax {
                            min: None,
                            max: None,
                        },
                    }),
                    networks: HashMap::new(),
                    info: serde_json::Value::Null,
                },
            );
        }

        Ok(currencies)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("Exchanges/{market_id}/Ticker");

        let response = self.public_request(&path).await?;
        let ticker: Bit2cTicker =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "Bit2cTicker".to_string(),
                message: e.to_string(),
            })?;

        let last = ticker
            .last
            .map(|l| Decimal::from_f64_retain(l).unwrap_or(Decimal::ZERO));
        let average = ticker
            .average
            .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO));
        let base_volume = ticker
            .base_volume
            .map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO));
        let bid = ticker
            .bid
            .map(|b| Decimal::from_f64_retain(b).unwrap_or(Decimal::ZERO));
        let ask = ticker
            .ask
            .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO));

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            high: None,
            low: None,
            bid,
            bid_volume: None,
            ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: last,
            last,
            previous_close: None,
            change: None,
            percentage: None,
            average,
            base_volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_tickers(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        Err(CcxtError::NotSupported {
            feature: "fetchTickers".to_string(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("Exchanges/{market_id}/orderbook");

        let response = self.public_request(&path).await?;
        let order_book: Bit2cOrderBook =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "Bit2cOrderBook".to_string(),
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

        // Apply limit if specified
        if let Some(lim) = limit {
            let lim = lim as usize;
            bids.truncate(lim);
            asks.truncate(lim);
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp: None,
            datetime: None,
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.get_market_id(symbol)?;
        let mut path = format!("Exchanges/{market_id}/trades");

        let mut query_parts = Vec::new();
        if let Some(s) = since {
            query_parts.push(format!("date={}", s / 1000));
        }
        if let Some(l) = limit {
            query_parts.push(format!("limit={l}"));
        }
        if !query_parts.is_empty() {
            // Note: For public endpoint we add query before .json extension
            path = format!("{}?{}", path, query_parts.join("&"));
        }

        // Remove .json for the call since public_request adds it
        let response = self.public_request(&path).await?;
        let trades_arr: Vec<Bit2cTrade> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "Vec<Bit2cTrade>".to_string(),
                message: e.to_string(),
            })?;

        let mut trades = Vec::new();
        for t in trades_arr {
            let timestamp = t.date.map(|d| d * 1000);
            let price = t
                .price
                .map(|p| Decimal::from_f64_retain(p).unwrap_or(Decimal::ZERO))
                .unwrap_or(Decimal::ZERO);
            let amount = t
                .amount
                .map(|a| Decimal::from_f64_retain(a).unwrap_or(Decimal::ZERO))
                .unwrap_or(Decimal::ZERO);

            let side = match t.is_bid {
                Some(true) => "buy",
                Some(false) => "sell",
                None => "buy",
            };

            trades.push(Trade {
                id: t.tid.map(|i| i.to_string()).unwrap_or_default(),
                order: None,
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp(ts / 1000, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(side.to_string()),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
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
        let response = self
            .private_get("Account/Balance/v2", &HashMap::new())
            .await?;

        let mut balances = HashMap::new();

        // Parse balance response
        if let Some(obj) = response.as_object() {
            let currencies = ["BTC", "ETH", "LTC", "USDC", "NIS"];
            for curr in currencies {
                let available_key = format!("AVAILABLE_{curr}");
                let total_key = curr.to_string();

                let free = obj
                    .get(&available_key)
                    .and_then(|v| v.as_f64())
                    .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO));
                let total = obj
                    .get(&total_key)
                    .and_then(|v| v.as_f64())
                    .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO));
                let used = free.zip(total).map(|(f, t)| t - f);

                if total.is_some() || free.is_some() {
                    balances.insert(
                        curr.to_string(),
                        Balance {
                            free,
                            used,
                            total,
                            debt: None,
                        },
                    );
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
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("Amount".to_string(), amount.to_string());
        params.insert("Pair".to_string(), market_id);

        let endpoint = match order_type {
            OrderType::Market => match side {
                OrderSide::Buy => "Order/AddOrderMarketPriceBuy",
                OrderSide::Sell => "Order/AddOrderMarketPriceSell",
            },
            OrderType::Limit => {
                let p = price.ok_or_else(|| CcxtError::BadRequest {
                    message: "Price required for limit order".to_string(),
                })?;
                params.insert("Price".to_string(), p.to_string());
                let total = amount * p;
                params.insert("Total".to_string(), total.to_string());
                params.insert("IsBid".to_string(), (side == OrderSide::Buy).to_string());
                "Order/AddOrder"
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?}"),
                });
            },
        };

        let response = self.private_post(endpoint, &params).await?;

        // Parse new order response
        let order_response: Bit2cOrderResponse =
            serde_json::from_value(response.clone()).map_err(|e| CcxtError::ParseError {
                data_type: "Bit2cOrderResponse".to_string(),
                message: e.to_string(),
            })?;

        if let Some(new_order) = order_response.new_order {
            return Ok(self.parse_new_order(&new_order, symbol, side, order_type));
        }

        // Fallback: return basic order
        Ok(Order {
            id: uuid::Uuid::new_v4().to_string(),
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

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), id.to_string());

        let response = self.private_post("Order/CancelOrder", &params).await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
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
        let mut params = HashMap::new();
        params.insert("id".to_string(), id.to_string());

        let response = self.private_get("Order/GetById", &params).await?;

        let order: Bit2cFetchOrder =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "Bit2cFetchOrder".to_string(),
                message: e.to_string(),
            })?;

        Ok(self.parse_order(&order, symbol))
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

        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("pair".to_string(), market_id.clone());

        let response = self.private_get("Order/MyOrders", &params).await?;

        let mut orders = Vec::new();

        // Parse response: { "BtcNis": { "ask": [...], "bid": [...] } }
        if let Some(pair_data) = response.get(&market_id) {
            if let Some(asks) = pair_data.get("ask").and_then(|a| a.as_array()) {
                for ask in asks {
                    if let Ok(order) = serde_json::from_value::<Bit2cFetchOrder>(ask.clone()) {
                        let mut parsed = self.parse_order(&order, symbol);
                        parsed.side = OrderSide::Sell;
                        orders.push(parsed);
                    }
                }
            }
            if let Some(bids) = pair_data.get("bid").and_then(|b| b.as_array()) {
                for bid in bids {
                    if let Ok(order) = serde_json::from_value::<Bit2cFetchOrder>(bid.clone()) {
                        let mut parsed = self.parse_order(&order, symbol);
                        parsed.side = OrderSide::Buy;
                        orders.push(parsed);
                    }
                }
            }
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
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(l) = limit {
            params.insert("take".to_string(), l.to_string());
        }

        if let Some(s) = since {
            // Format: YYYY.MM.DD
            let from_date = chrono::DateTime::from_timestamp(s / 1000, 0)
                .map(|dt| dt.format("%Y.%m.%d").to_string())
                .unwrap_or_default();
            let to_date = chrono::Utc::now().format("%Y.%m.%d").to_string();
            params.insert("fromTime".to_string(), from_date);
            params.insert("toTime".to_string(), to_date);
        }

        if let Some(sym) = symbol {
            let market_id = self.get_market_id(sym)?;
            params.insert("pair".to_string(), market_id);
        }

        let response = self.private_get("Order/OrderHistory", &params).await?;

        let trades_arr: Vec<serde_json::Value> =
            serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
                data_type: "Vec<Trade>".to_string(),
                message: e.to_string(),
            })?;

        let mut trades = Vec::new();
        for item in trades_arr {
            let timestamp = item.get("ticks").and_then(|t| t.as_i64()).map(|t| t * 1000);

            let price_str = item.get("price").and_then(|p| p.as_str()).unwrap_or("0");
            // Remove commas from price
            let price_clean = price_str.replace(",", "");
            let price = Decimal::from_str(&price_clean).unwrap_or(Decimal::ZERO);

            let amount_str = item
                .get("firstAmount")
                .and_then(|a| a.as_str())
                .unwrap_or("0");
            let amount = Decimal::from_str(amount_str).unwrap_or(Decimal::ZERO).abs();

            let action = item.get("action").and_then(|a| a.as_i64());
            let side = match action {
                Some(0) => "buy",
                Some(1) => "sell",
                _ => "buy",
            };

            let reference = item.get("reference").and_then(|r| r.as_str()).unwrap_or("");

            let is_maker = item.get("isMaker").and_then(|m| m.as_bool());
            let taker_or_maker = match is_maker {
                Some(true) => Some(TakerOrMaker::Maker),
                Some(false) => Some(TakerOrMaker::Taker),
                None => None,
            };

            // Parse order ID from reference: "pair|taker_order_id|maker_order_id"
            let parts: Vec<&str> = reference.split('|').collect();
            let order_id = if parts.len() >= 3 {
                if is_maker.unwrap_or(false) {
                    Some(parts[2].to_string())
                } else {
                    Some(parts[1].to_string())
                }
            } else {
                None
            };

            let fee_cost = item
                .get("feeAmount")
                .and_then(|f| f.as_str())
                .and_then(|s| Decimal::from_str(s).ok());

            let trade_symbol = symbol.unwrap_or("BTC/NIS");

            trades.push(Trade {
                id: reference.to_string(),
                order: order_id,
                timestamp,
                datetime: timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp(ts / 1000, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                symbol: trade_symbol.to_string(),
                trade_type: None,
                side: Some(side.to_string()),
                taker_or_maker,
                price,
                amount,
                cost: Some(price * amount),
                fee: fee_cost.map(|cost| Fee {
                    cost: Some(cost),
                    currency: Some("NIS".to_string()),
                    rate: None,
                }),
                fees: vec![],
                info: item,
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
        let exchange = Bit2c::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Bit2c);
        assert_eq!(exchange.name(), "Bit2C");
        assert_eq!(exchange.countries(), &["IL"]);
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().fetch_order_book);
        assert!(exchange.has().fetch_trades);
        assert!(!exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_hardcoded_markets() {
        let config = ExchangeConfig::default();
        let exchange = Bit2c::new(config).unwrap();

        let markets = exchange.get_hardcoded_markets();
        assert_eq!(markets.len(), 4);

        let btc_market = markets.iter().find(|m| m.symbol == "BTC/NIS").unwrap();
        assert_eq!(btc_market.id, "BtcNis");
        assert_eq!(btc_market.base, "BTC");
        assert_eq!(btc_market.quote, "NIS");
        assert!(btc_market.spot);
    }

    #[test]
    fn test_market_id_conversion() {
        let config = ExchangeConfig::default();
        let exchange = Bit2c::new(config).unwrap();

        // Load markets first
        let markets = exchange.get_hardcoded_markets();
        {
            let mut markets_lock = exchange.markets.write().unwrap();
            for m in markets {
                markets_lock.insert(m.symbol.clone(), m);
            }
        }

        let market_id = exchange.get_market_id("BTC/NIS").unwrap();
        assert_eq!(market_id, "BtcNis");
    }

    #[test]
    fn test_rate_limit() {
        let config = ExchangeConfig::default();
        let exchange = Bit2c::new(config).unwrap();
        assert_eq!(exchange.rate_limit(), 3000);
    }
}
