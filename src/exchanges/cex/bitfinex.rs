//! Bitfinex Exchange Implementation
//!
//! Major cryptocurrency exchange with advanced trading features

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha384;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha384 = Hmac<Sha384>;

/// Bitfinex exchange
pub struct Bitfinex {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Bitfinex {
    const BASE_URL: &'static str = "https://api-pub.bitfinex.com/v2";
    const PRIVATE_URL: &'static str = "https://api.bitfinex.com/v2";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// Create new Bitfinex instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: true,
            swap: true,
            future: false,
            option: false,
            fetch_markets: true,
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
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766244-e328a50c-5ed2-11e7-947b-041416579bb3.jpg".into()),
            api: api_urls,
            www: Some("https://www.bitfinex.com".into()),
            doc: vec![
                "https://docs.bitfinex.com/docs".into(),
                "https://github.com/bitfinexcom/bitfinex-api-node".into(),
            ],
            fees: Some("https://www.bitfinex.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1D".into());
        timeframes.insert(Timeframe::Week1, "1W".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

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

    /// Public API call
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API call
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let nonce = Utc::now().timestamp_millis().to_string();
        let body_str = body
            .as_ref()
            .map(|b| serde_json::to_string(b).unwrap_or_default())
            .unwrap_or_default();

        let signature_payload = format!("/api{path}{nonce}{body_str}");

        let mut mac = HmacSha384::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(signature_payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("bfx-nonce".into(), nonce);
        headers.insert("bfx-apikey".into(), api_key.to_string());
        headers.insert("bfx-signature".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        let private_client = HttpClient::new(Self::PRIVATE_URL, &self.config)?;
        private_client.post(path, body, Some(headers)).await
    }

    /// Convert symbol to market ID (BTC/USD -> tBTCUSD)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        format!("t{}", symbol.replace("/", ""))
    }

    /// Parse market data from API response
    fn parse_symbol(&self, market_id: &str) -> Option<(String, String, String)> {
        if !market_id.starts_with('t') {
            return None;
        }

        let s = &market_id[1..];

        // Try to parse with colon separator (e.g., BTC:USD)
        if s.contains(':') {
            let parts: Vec<&str> = s.split(':').collect();
            if parts.len() == 2 {
                let base = parts[0].to_string();
                let quote = parts[1].to_string();
                let symbol = format!("{base}/{quote}");
                return Some((symbol, base, quote));
            }
        }

        // Standard format: first 3 chars are base, rest is quote
        if s.len() >= 6 {
            let base = s[..3].to_string();
            let quote = s[3..].to_string();
            let symbol = format!("{base}/{quote}");
            return Some((symbol, base, quote));
        }

        None
    }
}

#[async_trait]
impl Exchange for Bitfinex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitfinex
    }

    fn name(&self) -> &str {
        "Bitfinex"
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
            let cache = self.markets.read().unwrap();
            if !reload && !cache.is_empty() {
                return Ok(cache.clone());
            }
        }

        let markets = self.fetch_markets().await?;
        let mut result = HashMap::new();

        let mut cache = self.markets.write().unwrap();
        let mut by_id = self.markets_by_id.write().unwrap();

        for market in markets {
            result.insert(market.symbol.clone(), market.clone());
            cache.insert(market.symbol.clone(), market.clone());
            by_id.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(result)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let cache = self.markets.read().unwrap();
        cache.get(symbol).map(|m| m.id.clone())
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let by_id = self.markets_by_id.read().unwrap();
        by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        // Fetch trading symbols
        let symbols_response: Vec<String> = self
            .public_get("/conf/pub:list:pair:exchange", None)
            .await?;

        let mut markets = Vec::new();

        for market_id in symbols_response {
            if let Some((symbol, base, quote)) = self.parse_symbol(&market_id) {
                let is_derivative = market_id.contains("F0");

                let market = Market {
                    id: market_id.clone(),
                    lowercase_id: Some(market_id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: base.clone(),
                    quote_id: quote.clone(),
                    market_type: if is_derivative {
                        MarketType::Swap
                    } else {
                        MarketType::Spot
                    },
                    spot: !is_derivative,
                    margin: true,
                    swap: is_derivative,
                    future: false,
                    option: false,
                    index: false,
                    active: true,
                    contract: is_derivative,
                    linear: if is_derivative { Some(true) } else { None },
                    inverse: if is_derivative { Some(false) } else { None },
                    sub_type: None,
                    taker: Some(Decimal::new(2, 3)), // 0.2%
                    maker: Some(Decimal::new(1, 3)), // 0.1%
                    contract_size: if is_derivative {
                        Some(Decimal::ONE)
                    } else {
                        None
                    },
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
            underlying: None,
            underlying_id: None,
                    settle: None,
                    settle_id: None,
                    precision: MarketPrecision {
                        amount: Some(8),
                        price: Some(5),
                        cost: None,
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits::default(),
                    margin_modes: None,
                    created: None,
                    info: serde_json::to_value(&market_id).unwrap_or_default(),
                    tier_based: false,
                    percentage: true,
                };
                markets.push(market);
            }
        }

        // Cache markets
        let mut cache = self.markets.write().unwrap();
        let mut by_id = self.markets_by_id.write().unwrap();
        for market in &markets {
            cache.insert(market.symbol.clone(), market.clone());
            by_id.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/ticker/{market_id}");
        let response: Vec<Decimal> = self.public_get(&path, None).await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: response.get(8).copied(),
            low: response.get(9).copied(),
            bid: response.first().copied(),
            bid_volume: response.get(1).copied(),
            ask: response.get(2).copied(),
            ask_volume: response.get(3).copied(),
            vwap: None,
            open: None,
            close: response.get(6).copied(),
            last: response.get(6).copied(),
            previous_close: None,
            change: response.get(4).copied(),
            percentage: response.get(5).map(|p| p * Decimal::from(100)),
            average: None,
            base_volume: response.get(7).copied(),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let precision = "P0";
        let path = format!("/book/{market_id}/{precision}");

        let mut params = HashMap::new();
        params.insert("len".into(), limit.unwrap_or(25).to_string());

        let response: Vec<Vec<Decimal>> = self.public_get(&path, Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for entry in &response {
            if entry.len() >= 3 {
                let price = entry[0];
                let count = entry[1];
                let amount = entry[2];

                // Count > 0 means valid entry, amount sign indicates side
                if count > Decimal::ZERO {
                    let entry = OrderBookEntry {
                        price,
                        amount: amount.abs(),
                    };

                    if amount > Decimal::ZERO {
                        bids.push(entry);
                    } else {
                        asks.push(entry);
                    }
                }
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
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
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/trades/{market_id}/hist");

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(120).to_string());

        let response: Vec<BitfinexTrade> = self.public_get(&path, Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let side = if t.amount > Decimal::ZERO {
                    "buy"
                } else {
                    "sell"
                };
                let amount_abs = t.amount.abs();

                Trade {
                    id: t.id.to_string(),
                    order: None,
                    timestamp: Some(t.mts),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.mts)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(side.to_string()),
                    taker_or_maker: None,
                    price: t.price,
                    amount: amount_abs,
                    cost: Some(t.price * amount_abs),
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
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.convert_to_market_id(symbol);
        let tf = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {timeframe:?}"),
            })?;
        let path = format!("/candles/trade:{tf}:{market_id}/hist");

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(100).to_string());

        let response: Vec<Vec<Decimal>> = self.public_get(&path, Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() >= 6 {
                    Some(OHLCV {
                        timestamp: c[0].to_string().parse().ok()?,
                        open: c[1],
                        close: c[2],
                        high: c[3],
                        low: c[4],
                        volume: c[5],
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<BitfinexWallet> = self.private_post("/auth/r/wallets", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for wallet in &response {
            let currency = wallet.currency.to_uppercase();
            let total = wallet.balance;
            let available = wallet.balance_available.unwrap_or(wallet.balance);

            balances.currencies.insert(
                currency,
                Balance {
                    free: Some(available),
                    used: Some(total - available),
                    total: Some(total),
                    debt: None,
                },
            );
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
        let market_id = self.convert_to_market_id(symbol);

        let amount_signed = match side {
            OrderSide::Buy => amount,
            OrderSide::Sell => -amount,
        };

        let type_str = match order_type {
            OrderType::Limit => "EXCHANGE LIMIT",
            OrderType::Market => "EXCHANGE MARKET",
            OrderType::StopLimit => "EXCHANGE STOP LIMIT",
            OrderType::StopMarket => "EXCHANGE STOP",
            _ => "EXCHANGE LIMIT",
        };

        let mut body = serde_json::json!({
            "type": type_str,
            "symbol": market_id,
            "amount": amount_signed.to_string(),
        });

        if let Some(p) = price {
            body["price"] = serde_json::json!(p.to_string());
        }

        let response: BitfinexOrderResponse = self
            .private_post("/auth/w/order/submit", Some(body))
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: None,
            timestamp: Some(timestamp),
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
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let body = serde_json::json!({
            "id": id.parse::<i64>().unwrap_or(0),
        });

        let _response: serde_json::Value = self
            .private_post("/auth/w/order/cancel", Some(body))
            .await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
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
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let body = serde_json::json!({
            "id": [id.parse::<i64>().unwrap_or(0)],
        });

        let response: Vec<BitfinexOrderDetail> =
            self.private_post("/auth/r/orders", Some(body)).await?;

        if let Some(order) = response.first() {
            let amount_abs = order.amount_orig.abs();
            let remaining_abs = order.amount.abs();
            let filled = amount_abs - remaining_abs;

            let status = match order.status.as_deref() {
                Some("ACTIVE") => OrderStatus::Open,
                Some("EXECUTED") | Some("FULLY EXECUTED") => OrderStatus::Closed,
                Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
                _ => OrderStatus::Open,
            };

            Ok(Order {
                id: order.id.to_string(),
                client_order_id: order.cid.clone(),
                timestamp: order.mts_create,
                datetime: order.mts_create.and_then(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                }),
                last_trade_timestamp: None,
                last_update_timestamp: order.mts_update,
                status,
                symbol: order.symbol.clone().unwrap_or_default(),
                order_type: OrderType::Limit,
                time_in_force: None,
                side: if order.amount_orig > Decimal::ZERO {
                    OrderSide::Buy
                } else {
                    OrderSide::Sell
                },
                price: order.price,
                average: order.price_avg,
                amount: amount_abs,
                filled,
                remaining: Some(remaining_abs),
                cost: None,
                trades: Vec::new(),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(order).unwrap_or_default(),
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                reduce_only: None,
                post_only: None,
            })
        } else {
            Err(CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
        }
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let response: Vec<BitfinexOrderDetail> = self.private_post("/auth/r/orders", None).await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|order| {
                let amount_abs = order.amount_orig.abs();
                let remaining_abs = order.amount.abs();
                let filled = amount_abs - remaining_abs;

                Order {
                    id: order.id.to_string(),
                    client_order_id: order.cid.clone(),
                    timestamp: order.mts_create,
                    datetime: order.mts_create.and_then(|ts| {
                        chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                    }),
                    last_trade_timestamp: None,
                    last_update_timestamp: order.mts_update,
                    status: OrderStatus::Open,
                    symbol: order.symbol.clone().unwrap_or_default(),
                    order_type: OrderType::Limit,
                    time_in_force: None,
                    side: if order.amount_orig > Decimal::ZERO {
                        OrderSide::Buy
                    } else {
                        OrderSide::Sell
                    },
                    price: order.price,
                    average: order.price_avg,
                    amount: amount_abs,
                    filled,
                    remaining: Some(remaining_abs),
                    cost: None,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(order).unwrap_or_default(),
                    stop_price: None,
                    trigger_price: None,
                    take_profit_price: None,
                    stop_loss_price: None,
                    reduce_only: None,
                    post_only: None,
                }
            })
            .collect();

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct BitfinexTrade {
    #[serde(default, rename = "0")]
    id: i64,
    #[serde(default, rename = "1")]
    mts: i64,
    #[serde(default, rename = "2")]
    amount: Decimal,
    #[serde(default, rename = "3")]
    price: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitfinexWallet {
    #[serde(default, rename = "0")]
    wallet_type: String,
    #[serde(default, rename = "1")]
    currency: String,
    #[serde(default, rename = "2")]
    balance: Decimal,
    #[serde(default, rename = "3")]
    unsettled_interest: Option<Decimal>,
    #[serde(default, rename = "4")]
    balance_available: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitfinexOrderResponse {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    gid: Option<i64>,
    #[serde(default)]
    cid: Option<i64>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    mts_create: Option<i64>,
    #[serde(default)]
    mts_update: Option<i64>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    amount_orig: Option<Decimal>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    type_prev: Option<String>,
    #[serde(default)]
    flags: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitfinexOrderDetail {
    #[serde(default, rename = "0")]
    id: i64,
    #[serde(default, rename = "1")]
    gid: Option<i64>,
    #[serde(default, rename = "2")]
    cid: Option<String>,
    #[serde(default, rename = "3")]
    symbol: Option<String>,
    #[serde(default, rename = "4")]
    mts_create: Option<i64>,
    #[serde(default, rename = "5")]
    mts_update: Option<i64>,
    #[serde(default, rename = "6")]
    amount: Decimal,
    #[serde(default, rename = "7")]
    amount_orig: Decimal,
    #[serde(default, rename = "8")]
    order_type: Option<String>,
    #[serde(default, rename = "13")]
    status: Option<String>,
    #[serde(default, rename = "16")]
    price: Option<Decimal>,
    #[serde(default, rename = "17")]
    price_avg: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitfinex::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitfinex);
        assert_eq!(exchange.name(), "Bitfinex");
        assert!(exchange.has().spot);
        assert!(exchange.has().margin);
        assert!(exchange.has().swap);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bitfinex::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USD"), "tBTCUSD");
        assert_eq!(exchange.convert_to_market_id("ETH/USDT"), "tETHUSDT");
    }

    #[test]
    fn test_parse_symbol() {
        let config = ExchangeConfig::new();
        let exchange = Bitfinex::new(config).unwrap();

        if let Some((symbol, base, quote)) = exchange.parse_symbol("tBTCUSD") {
            assert_eq!(symbol, "BTC/USD");
            assert_eq!(base, "BTC");
            assert_eq!(quote, "USD");
        }

        if let Some((symbol, base, quote)) = exchange.parse_symbol("tBTC:USD") {
            assert_eq!(symbol, "BTC/USD");
            assert_eq!(base, "BTC");
            assert_eq!(quote, "USD");
        }
    }
}
