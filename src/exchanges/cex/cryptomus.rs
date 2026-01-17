//! Cryptomus Exchange Implementation
//!
//! CCXT cryptomus.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use once_cell::sync::Lazy;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade,
};

// Empty timeframes since this exchange doesn't support OHLCV
static EMPTY_TIMEFRAMES: Lazy<HashMap<Timeframe, String>> = Lazy::new(HashMap::new);

/// Cryptomus 거래소
pub struct Cryptomus {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
}

impl Cryptomus {
    const BASE_URL: &'static str = "https://api.cryptomus.com";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 Cryptomus 인스턴스 생성
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
            fetch_ticker: false,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false,
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: false,
            fetch_trading_fees: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/8e0b1c48-7c01-4177-9224-f1b01d89d7e7"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://cryptomus.com".into()),
            doc: vec!["https://doc.cryptomus.com/personal".into()],
            fees: Some("https://cryptomus.com/tariffs".into()),
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
        })
    }

    /// 공개 API 호출
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

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let uid = self
            .config
            .uid()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "UID required".into(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let mut headers = HashMap::new();
        headers.insert("userId".into(), uid.to_string());

        let url = if method == "GET" {
            if params.is_empty() {
                path.to_string()
            } else {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                format!("{path}?{query}")
            }
        } else {
            path.to_string()
        };

        // 서명 생성 (POST/DELETE의 경우)
        let body_json = if method != "GET" {
            headers.insert("Content-Type".into(), "application/json".into());
            Some(serde_json::to_value(&params).unwrap_or_default())
        } else {
            None
        };

        let json_params_str = if let Some(ref json_val) = body_json {
            serde_json::to_string(json_val).unwrap_or_else(|_| "{}".to_string())
        } else {
            String::new()
        };
        let json_params_base64 = BASE64.encode(&json_params_str);
        let string_to_sign = format!("{json_params_base64}{secret}");

        let digest = md5::compute(string_to_sign.as_bytes());
        let signature = format!("{digest:x}");

        headers.insert("sign".into(), signature);

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                self.private_client
                    .post(&url, body_json, Some(headers))
                    .await
            },
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTC_USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// 마켓 ID → 심볼 변환 (BTC_USDT → BTC/USDT)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("_", "/")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &CryptomusTicker, symbol: &str) -> Ticker {
        Ticker {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            high: None,
            low: None,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_price,
            last: data.last_price,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.base_volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &CryptomusOrder, symbol: &str) -> Order {
        let status = match data.state.as_deref() {
            Some("active") => OrderStatus::Open,
            Some("completed") => OrderStatus::Closed,
            Some("partially_completed") => OrderStatus::Open,
            Some("cancelled") => OrderStatus::Canceled,
            Some("expired") => OrderStatus::Expired,
            Some("failed") => OrderStatus::Rejected,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.direction.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let timestamp = data.created_at.as_ref().and_then(|dt| {
            chrono::NaiveDateTime::parse_from_str(dt, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|ndt| ndt.and_utc().timestamp_millis())
        });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Option<Decimal> = data.quantity.as_ref().and_then(|q| q.parse().ok());
        let filled: Option<Decimal> = data.filled_quantity.as_ref().and_then(|f| f.parse().ok());
        let cost: Option<Decimal> = data.filled_value.as_ref().and_then(|v| v.parse().ok());

        let average = if let (Some(c), Some(f)) = (cost, filled) {
            if f > Decimal::ZERO {
                Some(c / f)
            } else {
                None
            }
        } else {
            None
        };

        let remaining = if let (Some(a), Some(f)) = (amount, filled) {
            Some(a - f)
        } else {
            None
        };

        Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: data.created_at.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount: amount.unwrap_or_default(),
            filled: filled.unwrap_or_default(),
            remaining,
            stop_price: data.stop_loss_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.stop_loss_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: None,
            stop_loss_price: data.stop_loss_price.as_ref().and_then(|p| p.parse().ok()),
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[CryptomusBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.available.parse().ok();
            let used: Option<Decimal> = b.held.parse().ok();
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
            result.add(&b.ticker, balance);
        }

        result
    }

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &CryptomusTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp * 1000; // Convert to milliseconds

        Trade {
            id: data.trade_id.clone(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.trade_type.clone(),
            taker_or_maker: None,
            price: data.price.parse().unwrap_or_default(),
            amount: data.quote_volume.parse().unwrap_or_default(),
            cost: Some(data.base_volume.parse().unwrap_or_default()),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Cryptomus {
    fn id(&self) -> ExchangeId {
        ExchangeId::Cryptomus
    }

    fn name(&self) -> &str {
        "Cryptomus"
    }

    fn version(&self) -> &str {
        "v2"
    }

    fn countries(&self) -> &[&str] {
        &["CA"]
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
        &EMPTY_TIMEFRAMES
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
        let response: CryptomusMarketsResponse = self
            .public_get("/v2/user-api/exchange/markets", None)
            .await?;

        let mut markets = Vec::new();

        for market_data in response.result {
            let parts: Vec<&str> = market_data.symbol.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base = parts[0].to_string();
            let quote = parts[1].to_string();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: market_data.symbol.clone(),
                lowercase_id: Some(market_data.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: market_data.base_currency.clone(),
                quote_id: market_data.quote_currency.clone(),
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
                taker: Some(Decimal::new(2, 3)), // 0.02% from fees config
                maker: Some(Decimal::new(2, 3)), // 0.02%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                precision: MarketPrecision {
                    amount: market_data.quote_prec.parse().ok(),
                    price: market_data.base_prec.parse().ok(),
                    cost: None,
                    base: market_data.base_prec.parse().ok(),
                    quote: market_data.quote_prec.parse().ok(),
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: market_data.quote_min_size.parse().ok(),
                        max: market_data.quote_max_size.parse().ok(),
                    },
                    price: MinMax {
                        min: market_data.base_min_size.parse().ok(),
                        max: market_data.base_max_size.parse().ok(),
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
                info: serde_json::to_value(&market_data).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, _symbol: &str) -> CcxtResult<Ticker> {
        Err(CcxtError::NotSupported {
            feature: "fetchTicker".into(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CryptomusTickersResponse =
            self.public_get("/v1/exchange/market/tickers", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response.data {
            if let Some(symbol) = markets_by_id.get(&data.currency_pair) {
                // If symbols filter is specified, check if this symbol is included
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

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/v1/exchange/market/order-book/{market_id}");

        let mut params = HashMap::new();
        params.insert("level".into(), "0".into());

        let response: CryptomusOrderBookResponse = self.public_get(&path, Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .data
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price.parse().unwrap_or_default(),
                amount: b.quantity.parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .data
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price.parse().unwrap_or_default(),
                amount: a.quantity.parse().unwrap_or_default(),
            })
            .collect();

        let timestamp = response
            .data
            .timestamp
            .parse::<i64>()
            .ok()
            .map(|t| t * 1000);

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
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
        let market_id = self.to_market_id(symbol);
        let path = format!("/v1/exchange/market/trades/{market_id}");

        let response: CryptomusTradesResponse = self.public_get(&path, None).await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetchOHLCV".into(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: CryptomusBalanceResponse = self
            .private_request("GET", "/v2/user-api/exchange/account/balance", params)
            .await?;

        Ok(self.parse_balance(&response.result))
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

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert(
            "direction".into(),
            match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }
            .into(),
        );
        params.insert("tag".into(), "ccxt".into());

        let path = match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("quantity".into(), amount.to_string());
                params.insert("price".into(), price_val.to_string());
                "/v2/user-api/exchange/orders"
            },
            OrderType::Market => {
                if side == OrderSide::Buy {
                    // For market buy, use value (cost in quote currency)
                    let cost = if let Some(p) = price {
                        amount * p
                    } else {
                        return Err(CcxtError::ArgumentsRequired {
                            message: "Market buy order requires price to calculate cost".into(),
                        });
                    };
                    params.insert("value".into(), cost.to_string());
                } else {
                    // For market sell, use quantity
                    params.insert("quantity".into(), amount.to_string());
                }
                "/v2/user-api/exchange/orders/market"
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            },
        };

        let response: CryptomusCreateOrderResponse =
            self.private_request("POST", path, params).await?;

        // Clone order_id before moving response
        let order_id = response.order_id.clone();

        // Return a minimal order structure with the order ID
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
        let path = format!("/v2/user-api/exchange/orders/{id}");
        let params = HashMap::new();

        let response: CryptomusCancelOrderResponse =
            self.private_request("DELETE", &path, params).await?;

        // Return minimal order structure indicating cancellation
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
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // Cryptomus doesn't have a direct fetch single order endpoint
        // We need to fetch open orders or history and filter by ID
        let open_orders = self.fetch_open_orders(Some(symbol), None, None).await?;

        if let Some(order) = open_orders.iter().find(|o| o.id == id) {
            return Ok(order.clone());
        }

        // If not in open orders, check history
        let params = HashMap::new();
        let response: CryptomusOrdersHistoryResponse = self
            .private_request("GET", "/v2/user-api/exchange/orders/history", params)
            .await?;

        for order_data in response.result {
            if order_data.id.as_deref() == Some(id) {
                return Ok(self.parse_order(&order_data, symbol));
            }
        }

        Err(CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("market".into(), self.to_market_id(s));
        }

        let response: CryptomusOpenOrdersResponse = self
            .private_request("GET", "/v2/user-api/exchange/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .result
            .iter()
            .map(|o| {
                let sym = o
                    .symbol
                    .as_ref()
                    .and_then(|s| markets_by_id.get(s))
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone().unwrap_or_default());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::NotSupported {
            feature: "fetchClosedOrders".into(),
        })
    }

    async fn fetch_my_trades(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        Err(CcxtError::NotSupported {
            feature: "fetchMyTrades".into(),
        })
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
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
            let uid = self.config.uid().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            headers.insert("userId".into(), uid.to_string());

            if method == "GET" && !params.is_empty() {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                url = format!("{url}?{query}");
            }

            let json_params = if method != "GET" {
                headers.insert("Content-Type".into(), "application/json".into());
                serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string())
            } else {
                String::new()
            };

            let json_params_base64 = BASE64.encode(&json_params);
            let string_to_sign = format!("{json_params_base64}{secret}");

            let digest = md5::compute(string_to_sign.as_bytes());
            let signature = format!("{digest:x}");

            headers.insert("sign".into(), signature);

            SignedRequest {
                url,
                method: method.to_string(),
                headers,
                body: if method != "GET" {
                    Some(json_params)
                } else {
                    None
                },
            }
        } else {
            if !params.is_empty() {
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
                body: None,
            }
        }
    }
}

// === Cryptomus API Response Types ===

#[derive(Debug, Deserialize)]
struct CryptomusMarketsResponse {
    result: Vec<CryptomusMarketData>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CryptomusMarketData {
    id: String,
    symbol: String,
    base_currency: String,
    quote_currency: String,
    base_min_size: String,
    quote_min_size: String,
    base_max_size: String,
    quote_max_size: String,
    base_prec: String,
    quote_prec: String,
}

#[derive(Debug, Deserialize)]
struct CryptomusTickersResponse {
    data: Vec<CryptomusTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptomusTicker {
    currency_pair: String,
    last_price: Option<Decimal>,
    base_volume: Option<Decimal>,
    quote_volume: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CryptomusOrderBookResponse {
    data: CryptomusOrderBookData,
}

#[derive(Debug, Deserialize)]
struct CryptomusOrderBookData {
    timestamp: String,
    bids: Vec<CryptomusOrderBookEntry>,
    asks: Vec<CryptomusOrderBookEntry>,
}

#[derive(Debug, Deserialize)]
struct CryptomusOrderBookEntry {
    price: String,
    quantity: String,
}

#[derive(Debug, Deserialize)]
struct CryptomusTradesResponse {
    data: Vec<CryptomusTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptomusTrade {
    trade_id: String,
    price: String,
    base_volume: String,
    quote_volume: String,
    timestamp: i64,
    #[serde(rename = "type")]
    trade_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CryptomusBalanceResponse {
    result: Vec<CryptomusBalance>,
}

#[derive(Debug, Deserialize)]
struct CryptomusBalance {
    ticker: String,
    available: String,
    held: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptomusCreateOrderResponse {
    order_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct CryptomusCancelOrderResponse {
    success: bool,
}

#[derive(Debug, Deserialize)]
struct CryptomusOpenOrdersResponse {
    result: Vec<CryptomusOrder>,
}

#[derive(Debug, Deserialize)]
struct CryptomusOrdersHistoryResponse {
    result: Vec<CryptomusOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CryptomusOrder {
    id: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    direction: Option<String>,
    symbol: Option<String>,
    price: Option<String>,
    quantity: Option<String>,
    filled_quantity: Option<String>,
    filled_value: Option<String>,
    state: Option<String>,
    created_at: Option<String>,
    client_order_id: Option<String>,
    stop_loss_price: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let cryptomus = Cryptomus::new(config).unwrap();

        assert_eq!(cryptomus.to_market_id("BTC/USDT"), "BTC_USDT");
        assert_eq!(cryptomus.to_symbol("BTC_USDT"), "BTC/USDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let cryptomus = Cryptomus::new(config).unwrap();

        assert_eq!(cryptomus.id(), ExchangeId::Cryptomus);
        assert_eq!(cryptomus.name(), "Cryptomus");
        assert!(cryptomus.has().spot);
        assert!(!cryptomus.has().margin);
        assert!(!cryptomus.has().swap);
    }
}
