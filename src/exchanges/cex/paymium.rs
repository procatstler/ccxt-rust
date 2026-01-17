//! Paymium Exchange Implementation
//!
//! CCXT paymium.ts ported to Rust
//! French/European Bitcoin exchange

#![allow(dead_code)]

use async_trait::async_trait;
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
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// Paymium Exchange
pub struct Paymium {
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

type HmacSha256 = Hmac<Sha256>;

impl Paymium {
    const BASE_URL: &'static str = "https://paymium.com/api/v1";
    const RATE_LIMIT_MS: u64 = 2000;

    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
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
            fetch_order: false,
            fetch_orders: false,
            fetch_open_orders: false,
            fetch_closed_orders: false,
            fetch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), "https://paymium.com/api/v1".into());
        api_urls.insert("private".into(), "https://paymium.com/api/v1".into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87153930-f0f02200-c2c0-11ea-9c0a-40337375ae89.jpg".into()),
            api: api_urls,
            www: Some("https://www.paymium.com".into()),
            doc: vec![
                "https://github.com/Paymium/api-documentation".into(),
                "https://www.paymium.com/page/developers".into(),
            ],
            fees: Some("https://www.paymium.com/page/help/fees".into()),
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
            nonce: RwLock::new(Utc::now().timestamp_millis()),
        })
    }

    fn get_nonce(&self) -> i64 {
        let mut nonce = self.nonce.write().unwrap();
        *nonce += 1;
        *nonce
    }

    /// Make public GET request
    async fn public_get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> CcxtResult<T> {
        let response = self.public_client.get(url, None, None).await?;
        serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
            message: e.to_string(),
            data_type: std::any::type_name::<T>().to_string(),
        })
    }

    /// Make private GET request
    async fn private_get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> CcxtResult<T> {
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

        let url = format!("{}{}", Self::BASE_URL, path);
        let nonce = self.get_nonce().to_string();
        let auth = format!("{nonce}{url}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(auth.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Api-Key".to_string(), api_key.to_string());
        headers.insert("Api-Nonce".to_string(), nonce);
        headers.insert("Api-Signature".to_string(), signature);

        let response = self.public_client.get(&url, Some(headers), None).await?;
        serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
            message: e.to_string(),
            data_type: std::any::type_name::<T>().to_string(),
        })
    }

    /// Make private POST request
    async fn private_post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
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

        let url = format!("{}{}", Self::BASE_URL, path);
        let nonce = self.get_nonce().to_string();

        let body = if params.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&params).unwrap_or_default()
        };

        let auth = format!("{nonce}{url}{body}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(auth.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Api-Key".to_string(), api_key.to_string());
        headers.insert("Api-Nonce".to_string(), nonce);
        headers.insert("Api-Signature".to_string(), signature);
        if !params.is_empty() {
            headers.insert("Content-Type".to_string(), "application/json".to_string());
        }

        let body_json: serde_json::Value = if params.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::to_value(&params)?
        };

        let response = self
            .private_client
            .post(&url, Some(body_json), Some(headers))
            .await?;
        serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
            message: e.to_string(),
            data_type: std::any::type_name::<T>().to_string(),
        })
    }

    /// Make private DELETE request
    async fn private_delete<T: for<'de> Deserialize<'de>>(&self, path: &str) -> CcxtResult<T> {
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

        let url = format!("{}{}", Self::BASE_URL, path);
        let nonce = self.get_nonce().to_string();
        let auth = format!("{nonce}{url}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(auth.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Api-Key".to_string(), api_key.to_string());
        headers.insert("Api-Nonce".to_string(), nonce);
        headers.insert("Api-Signature".to_string(), signature);

        let response = self
            .private_client
            .delete(&url, Some(headers), None)
            .await?;
        serde_json::from_value(response).map_err(|e| CcxtError::ParseError {
            message: e.to_string(),
            data_type: std::any::type_name::<T>().to_string(),
        })
    }

    fn parse_ticker(&self, ticker: &PaymiumTicker, symbol: &str) -> Ticker {
        let timestamp = ticker.at.map(|t| t * 1000);
        let base_volume = ticker
            .volume
            .as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let vwap = ticker.vwap.as_ref().and_then(|v| Decimal::from_str(v).ok());

        let quote_volume = match (base_volume, vwap) {
            (Some(bv), Some(vw)) => Some(bv * vw),
            _ => None,
        };

        let last = ticker
            .price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .unwrap()
                    .to_rfc3339()
            }),
            high: ticker.high.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            low: ticker.low.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid: ticker.bid.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            bid_volume: None,
            ask: ticker.ask.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            ask_volume: None,
            vwap,
            open: ticker.open.as_ref().and_then(|v| Decimal::from_str(v).ok()),
            close: last,
            last,
            previous_close: None,
            change: None,
            percentage: ticker
                .variation
                .as_ref()
                .and_then(|v| Decimal::from_str(v).ok()),
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker).unwrap_or_default(),
        }
    }

    fn parse_trade(&self, trade: &PaymiumTrade, symbol: &str) -> Trade {
        let timestamp = trade.created_at_int.map(|t| t * 1000);

        Trade {
            id: trade.uuid.clone().unwrap_or_default(),
            order: None,
            info: serde_json::to_value(trade).unwrap_or_default(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .unwrap()
                    .to_rfc3339()
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            taker_or_maker: None,
            side: trade.side.as_ref().map(|s| {
                if s == "buy" {
                    "buy".to_string()
                } else {
                    "sell".to_string()
                }
            }),
            price: trade
                .price
                .as_ref()
                .and_then(|p| Decimal::from_str(p).ok())
                .unwrap_or_default(),
            amount: trade
                .traded_btc
                .as_ref()
                .and_then(|a| Decimal::from_str(a).ok())
                .unwrap_or_default(),
            cost: None,
            fee: None,
            fees: Vec::new(),
        }
    }
}

#[async_trait]
impl Exchange for Paymium {
    fn id(&self) -> ExchangeId {
        ExchangeId::Paymium
    }

    fn name(&self) -> &str {
        "Paymium"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["FR", "EU"]
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
        static TIMEFRAMES: std::sync::OnceLock<HashMap<Timeframe, String>> =
            std::sync::OnceLock::new();
        TIMEFRAMES.get_or_init(HashMap::new)
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
        // Paymium only supports BTC/EUR
        let market = Market {
            id: "eur".to_string(),
            lowercase_id: Some("eur".to_string()),
            symbol: "BTC/EUR".to_string(),
            base: "BTC".to_string(),
            quote: "EUR".to_string(),
            base_id: "btc".to_string(),
            quote_id: "eur".to_string(),
            active: true,
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            contract: false,
            linear: None,
            inverse: None,
            settle: None,
            settle_id: None,
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            taker: Some(Decimal::from_str("0.005").unwrap()),
            maker: Some(Decimal::from_str("-0.001").unwrap()), // Negative maker fee (rebate)
            percentage: true,
            tier_based: false,
            index: false,
            precision: MarketPrecision {
                amount: Some(8),
                price: Some(2),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: Some(Decimal::from_str("0.0001").unwrap()),
                    max: None,
                },
                price: MinMax {
                    min: Some(Decimal::from_str("0.01").unwrap()),
                    max: None,
                },
                cost: MinMax {
                    min: Some(Decimal::ONE),
                    max: None,
                },
                leverage: MinMax::default(),
            },
            sub_type: None,
            margin_modes: None,
            created: None,
            info: serde_json::json!({
                "id": "eur",
                "symbol": "BTC/EUR",
                "base": "BTC",
                "quote": "EUR"
            }),
        };

        let mut markets = self.markets.write().unwrap();
        let mut markets_by_id = self.markets_by_id.write().unwrap();

        markets.insert(market.symbol.clone(), market.clone());
        markets_by_id.insert(market.id.clone(), market.symbol.clone());

        Ok(vec![market])
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        if symbol != "BTC/EUR" {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let url = format!("{}/data/eur/ticker", Self::BASE_URL);
        let ticker: PaymiumTicker = self.public_get(&url).await?;
        Ok(self.parse_ticker(&ticker, symbol))
    }

    async fn fetch_tickers(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let ticker = self.fetch_ticker("BTC/EUR").await?;
        let mut result = HashMap::new();
        result.insert("BTC/EUR".to_string(), ticker);
        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        if symbol != "BTC/EUR" {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let url = format!("{}/data/eur/depth", Self::BASE_URL);
        let response: PaymiumOrderBook = self.public_get(&url).await?;
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = response
            .bids
            .unwrap_or_default()
            .iter()
            .map(|entry| OrderBookEntry {
                price: entry
                    .price
                    .as_ref()
                    .and_then(|p| Decimal::from_str(p).ok())
                    .unwrap_or_default(),
                amount: entry
                    .amount
                    .as_ref()
                    .and_then(|a| Decimal::from_str(a).ok())
                    .unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .unwrap_or_default()
            .iter()
            .map(|entry| OrderBookEntry {
                price: entry
                    .price
                    .as_ref()
                    .and_then(|p| Decimal::from_str(p).ok())
                    .unwrap_or_default(),
                amount: entry
                    .amount
                    .as_ref()
                    .and_then(|a| Decimal::from_str(a).ok())
                    .unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        if symbol != "BTC/EUR" {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let url = format!("{}/data/eur/trades", Self::BASE_URL);
        let response: Vec<PaymiumTrade> = self.public_get(&url).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|trade| self.parse_trade(trade, symbol))
            .collect();

        Ok(trades)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: PaymiumUserResponse = self.private_get("/user").await?;

        let mut currencies = HashMap::new();

        // Parse BTC balance
        if let (Some(btc_free), Some(btc_locked)) = (&response.balance_btc, &response.locked_btc) {
            let free = Decimal::from_str(btc_free).unwrap_or_default();
            let used = Decimal::from_str(btc_locked).unwrap_or_default();
            currencies.insert(
                "BTC".to_string(),
                Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(free + used),
                    debt: None,
                },
            );
        }

        // Parse EUR balance
        if let (Some(eur_free), Some(eur_locked)) = (&response.balance_eur, &response.locked_eur) {
            let free = Decimal::from_str(eur_free).unwrap_or_default();
            let used = Decimal::from_str(eur_locked).unwrap_or_default();
            currencies.insert(
                "EUR".to_string(),
                Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(free + used),
                    debt: None,
                },
            );
        }

        Ok(Balances {
            info: serde_json::to_value(&response).unwrap_or_default(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies,
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
        if symbol != "BTC/EUR" {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let type_str = match order_type {
            OrderType::Limit => "LimitOrder",
            OrderType::Market => "MarketOrder",
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type {order_type:?}"),
                })
            },
        };

        let direction = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("type".to_string(), serde_json::json!(type_str));
        params.insert("currency".to_string(), serde_json::json!("eur"));
        params.insert("direction".to_string(), serde_json::json!(direction));
        params.insert("amount".to_string(), serde_json::json!(amount.to_string()));

        if order_type == OrderType::Limit {
            if let Some(p) = price {
                params.insert("price".to_string(), serde_json::json!(p.to_string()));
            } else {
                return Err(CcxtError::ArgumentsRequired {
                    message: "Price required for limit orders".into(),
                });
            }
        }

        let response: PaymiumOrderResponse = self.private_post("/user/orders", params).await?;

        Ok(Order {
            id: response.uuid.clone().unwrap_or_default(),
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
        let path = format!("/user/orders/{id}/cancel");
        let response: serde_json::Value = self.private_delete(&path).await?;

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
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: response,
        })
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv not supported on Paymium".into(),
        })
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            feature: "fetch_order not supported on Paymium".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_open_orders not supported on Paymium".into(),
        })
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        if symbol == "BTC/EUR" {
            Some("eur".to_string())
        } else {
            None
        }
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        if market_id == "eur" {
            Some("BTC/EUR".to_string())
        } else {
            None
        }
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
        let url = format!("{}{}", Self::BASE_URL, path);

        SignedRequest {
            url,
            method: if api == "public" { "GET" } else { "POST" }.into(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

// === Paymium API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumTicker {
    high: Option<String>,
    low: Option<String>,
    volume: Option<String>,
    bid: Option<String>,
    ask: Option<String>,
    midpoint: Option<String>,
    vwap: Option<String>,
    at: Option<i64>,
    price: Option<String>,
    open: Option<String>,
    variation: Option<String>,
    currency: Option<String>,
    trade_id: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumOrderBookEntry {
    price: Option<String>,
    amount: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaymiumOrderBook {
    bids: Option<Vec<PaymiumOrderBookEntry>>,
    asks: Option<Vec<PaymiumOrderBookEntry>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumTrade {
    uuid: Option<String>,
    side: Option<String>,
    price: Option<String>,
    traded_btc: Option<String>,
    traded_currency: Option<String>,
    created_at_int: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumUserResponse {
    name: Option<String>,
    email: Option<String>,
    balance_btc: Option<String>,
    locked_btc: Option<String>,
    balance_eur: Option<String>,
    locked_eur: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PaymiumOrderResponse {
    uuid: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    currency: Option<String>,
    direction: Option<String>,
    price: Option<String>,
    amount: Option<String>,
    state: Option<String>,
}
