//! Tokocrypto Exchange Implementation
//!
//! Indonesian cryptocurrency exchange (Binance partner)

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
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// Tokocrypto exchange
pub struct Tokocrypto {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Tokocrypto {
    const BASE_URL: &'static str = "https://www.tokocrypto.com/open/v1";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// Create new Tokocrypto instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: false,
            swap: false,
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
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/183870484-d3398d0c-f6a1-4cce-91b8-d58792308716.jpg".into()),
            api: api_urls,
            www: Some("https://www.tokocrypto.com/".into()),
            doc: vec!["https://www.tokocrypto.com/apidocs/".into()],
            fees: Some("https://www.tokocrypto.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
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
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
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

        let timestamp = Utc::now().timestamp_millis().to_string();

        let mut request_params = params.unwrap_or_default();
        request_params.insert("timestamp".into(), timestamp);

        let query_string: String = request_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        request_params.insert("signature".into(), signature);

        let mut headers = HashMap::new();
        headers.insert("X-MBX-APIKEY".into(), api_key.to_string());
        headers.insert(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        );

        if method == "GET" {
            let full_query: String = request_params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            let full_path = format!("{path}?{full_query}");
            self.client.get(&full_path, None, Some(headers)).await
        } else {
            self.client
                .post_form(path, &request_params, Some(headers))
                .await
        }
    }

    /// Get market ID from symbol
    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        markets
            .get(symbol)
            .map(|m| m.id.clone())
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
    }
}

#[async_trait]
impl Exchange for Tokocrypto {
    fn id(&self) -> ExchangeId {
        ExchangeId::Tokocrypto
    }

    fn name(&self) -> &str {
        "Tokocrypto"
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
        let response: TokocryptoMarketsResponse = self.public_get("/common/symbols", None).await?;

        let mut markets = Vec::new();
        for data in response.list {
            let symbol = format!("{}/{}", data.base_asset, data.quote_asset);
            let active = data.status == "TRADING";

            let market = Market {
                id: data.symbol.clone(),
                lowercase_id: Some(data.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: data.base_asset.clone(),
                quote: data.quote_asset.clone(),
                base_id: data.base_asset.clone(),
                quote_id: data.quote_asset.clone(),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(1, 3)), // 0.001 = 0.1%
                maker: Some(Decimal::new(1, 3)), // 0.001 = 0.1%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: data.base_asset_precision,
                    price: data.quote_precision,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::json!(&data),
                tier_based: false,
                percentage: true,
            };
            markets.push(market);
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
        let market_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: TokocryptoTicker =
            self.public_get("/market/ticker/24hr", Some(params)).await?;

        let timestamp = response
            .close_time
            .or_else(|| Some(Utc::now().timestamp_millis()));

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high: response.high_price,
            low: response.low_price,
            bid: response.bid_price,
            bid_volume: response.bid_qty,
            ask: response.ask_price,
            ask_volume: response.ask_qty,
            vwap: None,
            open: response.open_price,
            close: response.last_price,
            last: response.last_price,
            previous_close: response.prev_close_price,
            change: response.price_change,
            percentage: response.price_change_percent,
            average: None,
            base_volume: response.volume,
            quote_volume: response.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: TokocryptoOrderBook = self.public_get("/market/depth", Some(params)).await?;
        let timestamp = Some(Utc::now().timestamp_millis());

        let parse_entries = |entries: &Vec<Vec<Decimal>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: e[0],
                        amount: e[1],
                    })
                } else {
                    None
                }
            });
            if let Some(l) = limit {
                iter.take(l as usize).collect()
            } else {
                iter.collect()
            }
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: response.last_update_id,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
            checksum: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<TokocryptoTrade> =
            self.public_get("/market/trades", Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let timestamp = t.time;
                let side = if t.is_buyer_maker { "sell" } else { "buy" };

                Trade {
                    id: t.id.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(side.into()),
                    taker_or_maker: None,
                    price: t.price,
                    amount: t.qty,
                    cost: t.quote_qty,
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
        let market_id = self.get_market_id(symbol)?;
        let tf = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {timeframe:?} not supported"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), tf.clone());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<Vec<serde_json::Value>> =
            self.public_get("/market/klines", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|arr| {
                if arr.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: arr[0].as_i64()?,
                    open: arr[1].as_str().and_then(|s| s.parse().ok())?,
                    high: arr[2].as_str().and_then(|s| s.parse().ok())?,
                    low: arr[3].as_str().and_then(|s| s.parse().ok())?,
                    close: arr[4].as_str().and_then(|s| s.parse().ok())?,
                    volume: arr[5].as_str().and_then(|s| s.parse().ok())?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: TokocryptoBalanceResponse =
            self.private_request("GET", "/account/spot", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance in response.balances {
            let currency = balance.asset.to_uppercase();
            let free = balance.free.unwrap_or_default();
            let used = balance.locked.unwrap_or_default();

            if free > Decimal::ZERO || used > Decimal::ZERO {
                balances.currencies.insert(
                    currency,
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(free + used),
                        debt: None,
                    },
                );
            }
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
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "BUY".into(),
                OrderSide::Sell => "SELL".into(),
            },
        );
        params.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "LIMIT".into(),
                OrderType::Market => "MARKET".into(),
                _ => {
                    return Err(CcxtError::NotSupported {
                        feature: format!("Order type {order_type:?} not supported"),
                    })
                },
            },
        );
        params.insert("quantity".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let p = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price is required for limit orders".into(),
            })?;
            params.insert("price".into(), p.to_string());
            params.insert("timeInForce".into(), "GTC".into());
        }

        let response: TokocryptoOrder = self
            .private_request("POST", "/orders", Some(params))
            .await?;

        let timestamp = response
            .transact_time
            .or_else(|| Some(Utc::now().timestamp_millis()));

        Ok(Order {
            id: response.order_id.to_string(),
            client_order_id: response.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: self.parse_order_status(&response.status),
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None, // Tokocrypto uses string time_in_force
            side,
            price: response.price,
            average: None,
            amount: response.orig_qty.unwrap_or(amount),
            filled: response.executed_qty.unwrap_or_default(),
            remaining: response
                .orig_qty
                .map(|orig| orig - response.executed_qty.as_ref().copied().unwrap_or_default()),
            cost: response.cumulative_quote_qty,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: response.stop_price,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: TokocryptoOrder = self
            .private_request("POST", "/orders/cancel", Some(params))
            .await?;

        let timestamp = response
            .transact_time
            .or_else(|| Some(Utc::now().timestamp_millis()));

        Ok(Order {
            id: response.order_id.to_string(),
            client_order_id: response.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None, // Tokocrypto uses string time_in_force
            side: OrderSide::Buy,
            price: response.price,
            average: None,
            amount: response.orig_qty.unwrap_or_default(),
            filled: response.executed_qty.unwrap_or_default(),
            remaining: response
                .orig_qty
                .map(|orig| orig - response.executed_qty.as_ref().copied().unwrap_or_default()),
            cost: response.cumulative_quote_qty,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: response.stop_price,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: TokocryptoOrder = self
            .private_request("GET", "/orders/detail", Some(params))
            .await?;

        let timestamp = response.time;
        let status = self.parse_order_status(&response.status);

        let (order_type, side) =
            self.parse_order_type_and_side(&response.order_type, &response.side);

        Ok(Order {
            id: response.order_id.to_string(),
            client_order_id: response.client_order_id.clone(),
            timestamp,
            datetime: timestamp.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: response.update_time,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None, // Tokocrypto uses string time_in_force
            side,
            price: response.price,
            average: None,
            amount: response.orig_qty.unwrap_or_default(),
            filled: response.executed_qty.unwrap_or_default(),
            remaining: response
                .orig_qty
                .map(|orig| orig - response.executed_qty.as_ref().copied().unwrap_or_default()),
            cost: response.cumulative_quote_qty,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: response.stop_price,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        let symbol_str = if let Some(sym) = symbol {
            let market_id = self.get_market_id(sym)?;
            params.insert("symbol".into(), market_id);
            sym.to_string()
        } else {
            String::new()
        };

        let response: Vec<TokocryptoOrder> = self
            .private_request("GET", "/orders/open", Some(params))
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let timestamp = o.time;
                let status = self.parse_order_status(&o.status);
                let (order_type, side) = self.parse_order_type_and_side(&o.order_type, &o.side);

                Order {
                    id: o.order_id.to_string(),
                    client_order_id: o.client_order_id.clone(),
                    timestamp,
                    datetime: timestamp.map(|t| {
                        chrono::DateTime::from_timestamp_millis(t)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default()
                    }),
                    last_trade_timestamp: None,
                    last_update_timestamp: o.update_time,
                    status,
                    symbol: if symbol_str.is_empty() {
                        self.symbol(&o.symbol).unwrap_or_default()
                    } else {
                        symbol_str.clone()
                    },
                    order_type,
                    time_in_force: None, // Tokocrypto uses string time_in_force
                    side,
                    price: o.price,
                    average: None,
                    amount: o.orig_qty.unwrap_or_default(),
                    filled: o.executed_qty.unwrap_or_default(),
                    remaining: o
                        .orig_qty
                        .map(|orig| orig - o.executed_qty.as_ref().copied().unwrap_or_default()),
                    cost: o.cumulative_quote_qty,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                    stop_price: o.stop_price,
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

impl Tokocrypto {
    /// Parse order status from Tokocrypto status string
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "NEW" | "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "REJECTED" | "EXPIRED" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order type and side from Tokocrypto strings
    fn parse_order_type_and_side(&self, order_type: &str, side: &str) -> (OrderType, OrderSide) {
        let ot = match order_type.to_uppercase().as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let os = match side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        (ot, os)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize)]
struct TokocryptoMarketsResponse {
    #[serde(default)]
    list: Vec<TokocryptoMarket>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokocryptoMarket {
    #[serde(default)]
    symbol: String,
    #[serde(default, rename = "baseAsset")]
    base_asset: String,
    #[serde(default, rename = "quoteAsset")]
    quote_asset: String,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "baseAssetPrecision")]
    base_asset_precision: Option<i32>,
    #[serde(default, rename = "quotePrecision")]
    quote_precision: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokocryptoTicker {
    #[serde(default)]
    symbol: String,
    #[serde(default, rename = "lastPrice")]
    last_price: Option<Decimal>,
    #[serde(default, rename = "highPrice")]
    high_price: Option<Decimal>,
    #[serde(default, rename = "lowPrice")]
    low_price: Option<Decimal>,
    #[serde(default, rename = "bidPrice")]
    bid_price: Option<Decimal>,
    #[serde(default, rename = "bidQty")]
    bid_qty: Option<Decimal>,
    #[serde(default, rename = "askPrice")]
    ask_price: Option<Decimal>,
    #[serde(default, rename = "askQty")]
    ask_qty: Option<Decimal>,
    #[serde(default, rename = "openPrice")]
    open_price: Option<Decimal>,
    #[serde(default, rename = "prevClosePrice")]
    prev_close_price: Option<Decimal>,
    #[serde(default, rename = "priceChange")]
    price_change: Option<Decimal>,
    #[serde(default, rename = "priceChangePercent")]
    price_change_percent: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default, rename = "quoteVolume")]
    quote_volume: Option<Decimal>,
    #[serde(default, rename = "closeTime")]
    close_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TokocryptoOrderBook {
    #[serde(default, rename = "lastUpdateId")]
    last_update_id: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokocryptoTrade {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    qty: Decimal,
    #[serde(default, rename = "quoteQty")]
    quote_qty: Option<Decimal>,
    #[serde(default)]
    time: i64,
    #[serde(default, rename = "isBuyerMaker")]
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize)]
struct TokocryptoBalanceResponse {
    #[serde(default)]
    balances: Vec<TokocryptoBalance>,
}

#[derive(Debug, Deserialize)]
struct TokocryptoBalance {
    #[serde(default)]
    asset: String,
    #[serde(default)]
    free: Option<Decimal>,
    #[serde(default)]
    locked: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TokocryptoOrder {
    #[serde(default)]
    symbol: String,
    #[serde(default, rename = "orderId")]
    order_id: i64,
    #[serde(default, rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default, rename = "origQty")]
    orig_qty: Option<Decimal>,
    #[serde(default, rename = "executedQty")]
    executed_qty: Option<Decimal>,
    #[serde(default, rename = "cummulativeQuoteQty")]
    cumulative_quote_qty: Option<Decimal>,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "timeInForce")]
    time_in_force: Option<String>,
    #[serde(default, rename = "type")]
    order_type: String,
    #[serde(default)]
    side: String,
    #[serde(default, rename = "stopPrice")]
    stop_price: Option<Decimal>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default, rename = "updateTime")]
    update_time: Option<i64>,
    #[serde(default, rename = "transactTime")]
    transact_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Tokocrypto::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Tokocrypto);
        assert_eq!(exchange.name(), "Tokocrypto");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Tokocrypto::new(config).unwrap();
        assert_eq!(
            exchange.timeframes().get(&Timeframe::Minute1),
            Some(&"1m".to_string())
        );
        assert_eq!(
            exchange.timeframes().get(&Timeframe::Hour1),
            Some(&"1h".to_string())
        );
    }
}
