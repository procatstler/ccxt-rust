//! Independent Reserve Exchange Implementation
//!
//! CCXT independentreserve.ts를 Rust로 포팅

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
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade,
    Transaction, TransactionStatus, TransactionType, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Independent Reserve 거래소
pub struct Independentreserve {
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

impl Independentreserve {
    const PUBLIC_URL: &'static str = "https://api.independentreserve.com/Public";
    const PRIVATE_URL: &'static str = "https://api.independentreserve.com/Private";
    const RATE_LIMIT_MS: u64 = 1000; // 1000ms = 1 request per second

    /// 새 Independent Reserve 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::PUBLIC_URL, &config)?;
        let private_client = HttpClient::new(Self::PRIVATE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_balance: true,
            create_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposit_address: true,
            withdraw: true,
            fetch_trading_fees: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::PUBLIC_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87182090-1e9e9080-c2ec-11ea-8e49-563db9a38f37.jpg".into()),
            api: api_urls,
            www: Some("https://www.independentreserve.com".into()),
            doc: vec!["https://www.independentreserve.com/API".into()],
            fees: None,
        };

        let timeframes = HashMap::new();

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
            format!("/{path}?{query}")
        } else {
            format!("/{path}")
        };

        self.public_client.get(&url, None, None).await
    }

    /// 비공개 API 호출
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = Utc::now().timestamp_millis();
        let url = format!("{}{}", Self::PRIVATE_URL, path);

        // Build auth array: [url, apiKey=value, nonce=value, param1=value, ...]
        let mut auth_parts = vec![
            url.clone(),
            format!("apiKey={}", api_key),
            format!("nonce={}", nonce),
        ];

        // Sort params by key for consistent ordering
        let mut sorted_keys: Vec<_> = params.keys().collect();
        sorted_keys.sort();

        for key in sorted_keys {
            let value = &params[key];
            let value_str = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => value.to_string(),
            };
            auth_parts.push(format!("{}={}", key, value_str));
        }

        let message = auth_parts.join(",");

        // Create HMAC-SHA256 signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes()).to_uppercase();

        // Build request body
        let mut body = params.clone();
        body.insert("apiKey".into(), serde_json::Value::String(api_key.to_string()));
        body.insert("nonce".into(), serde_json::Value::Number(nonce.into()));
        body.insert("signature".into(), serde_json::Value::String(signature));

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());

        self.private_client
            .post(&format!("/{path}"), Some(serde_json::to_value(&body)?), Some(headers))
            .await
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &IndependentreserveTicker, symbol: &str) -> Ticker {
        let timestamp = data.created_timestamp_utc
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: data.created_timestamp_utc.clone(),
            high: data.day_highest_price,
            low: data.day_lowest_price,
            bid: data.current_highest_bid_price,
            bid_volume: None,
            ask: data.current_lowest_offer_price,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_price,
            last: data.last_price,
            previous_close: None,
            change: None,
            percentage: None,
            average: data.day_avg_price,
            base_volume: data.day_volume_xbt_in_secondary_currrency,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &IndependentreserveOrder, symbol: Option<&str>) -> Order {
        let status = match data.status.as_str() {
            "Open" => OrderStatus::Open,
            "PartiallyFilled" => OrderStatus::Open,
            "Filled" => OrderStatus::Closed,
            "PartiallyFilledAndCancelled" | "Cancelled" => OrderStatus::Canceled,
            "PartiallyFilledAndExpired" | "Expired" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type_str = data.order_type.as_ref().or(data.type_field.as_ref());
        let (order_type, side) = if let Some(ot) = order_type_str {
            let ot_lower = ot.to_lowercase();
            let side = if ot_lower.contains("bid") {
                OrderSide::Buy
            } else if ot_lower.contains("offer") {
                OrderSide::Sell
            } else {
                OrderSide::Buy
            };

            let order_type = if ot_lower.contains("market") {
                OrderType::Market
            } else {
                OrderType::Limit
            };

            (order_type, side)
        } else {
            (OrderType::Limit, OrderSide::Buy)
        };

        let timestamp = data.created_timestamp_utc
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let symbol = if let Some(s) = symbol {
            s.to_string()
        } else if let (Some(base), Some(quote)) = (&data.primary_currency_code, &data.secondary_currency_code) {
            format!("{}/{}", base, quote)
        } else {
            String::new()
        };

        let amount = data.volume_ordered.or(data.volume).unwrap_or_default();
        let filled = data.volume_filled.unwrap_or_default();
        let remaining = data.outstanding;

        Order {
            id: data.order_guid.clone().unwrap_or_default(),
            client_order_id: None,
            timestamp,
            datetime: data.created_timestamp_utc.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: data.price,
            average: data.avg_price,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data.value,
            trades: Vec::new(),
            fee: data.fee_percent.map(|rate| crate::types::Fee {
                rate: Some(rate),
                cost: Some(filled * rate),
                currency: data.primary_currency_code.clone(),
            }),
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &IndependentreserveTrade, symbol: Option<&str>) -> Trade {
        let timestamp = data.trade_timestamp_utc
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let symbol = if let Some(s) = symbol {
            s.to_string()
        } else if let (Some(base), Some(quote)) = (&data.primary_currency_code, &data.secondary_currency_code) {
            format!("{}/{}", base, quote)
        } else {
            String::new()
        };

        let side = data.order_type.as_ref().and_then(|ot| {
            let ot_lower = ot.to_lowercase();
            if ot_lower.contains("bid") {
                Some("buy".to_string())
            } else if ot_lower.contains("offer") {
                Some("sell".to_string())
            } else {
                None
            }
        });

        let price = data.price.or(data.secondary_currency_trade_price).unwrap_or_default();
        let amount = data.volume_traded.or(data.primary_currency_amount).unwrap_or_default();

        Trade {
            id: data.trade_guid.clone().unwrap_or_default(),
            order: data.order_guid.clone(),
            timestamp,
            datetime: data.trade_timestamp_utc.clone(),
            symbol,
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[IndependentreserveBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free = b.available_balance;
            let total = b.total_balance;
            let used = total.and_then(|t| free.map(|f| t - f));

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };

            if let Some(code) = &b.currency_code {
                result.add(code, balance);
            }
        }

        result
    }
}

#[async_trait]
impl Exchange for Independentreserve {
    fn id(&self) -> ExchangeId {
        ExchangeId::Independentreserve
    }

    fn name(&self) -> &str {
        "Independent Reserve"
    }

    fn version(&self) -> &str {
        "1"
    }

    fn countries(&self) -> &[&str] {
        &["AU", "NZ"]
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
        // Fetch base currencies, quote currencies and limits in parallel
        let base_currencies: Vec<String> = self
            .public_get("GetValidPrimaryCurrencyCodes", None)
            .await?;

        let quote_currencies: Vec<String> = self
            .public_get("GetValidSecondaryCurrencyCodes", None)
            .await?;

        let limits: HashMap<String, Decimal> = self
            .public_get("GetOrderMinimumVolumes", None)
            .await?;

        let mut markets = Vec::new();

        for base_id in &base_currencies {
            let base = base_id.to_uppercase();
            let min_amount = limits.get(base_id);

            for quote_id in &quote_currencies {
                let quote = quote_id.to_uppercase();
                let id = format!("{}/{}", base_id, quote_id);
                let symbol = format!("{}/{}", base, quote);

                let market = Market {
                    id: id.clone(),
                    lowercase_id: Some(id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: base_id.clone(),
                    quote_id: quote_id.clone(),
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
                    taker: Some(Decimal::new(5, 3)), // 0.5%
                    maker: Some(Decimal::new(5, 3)), // 0.5%
                    contract_size: None,
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
                    precision: MarketPrecision {
                        amount: None,
                        price: None,
                        cost: None,
                        base: None,
                        quote: None,
                    },
                    limits: MarketLimits {
                        amount: crate::types::MinMax {
                            min: min_amount.copied(),
                            max: None,
                        },
                        ..Default::default()
                    },
                    margin_modes: None,
                    created: None,
                    info: serde_json::Value::String(id),
                    tier_based: false,
                    percentage: true,
                };

                markets.push(market);
            }
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market = self.markets.read().unwrap()
            .get(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?
            .clone();

        let mut params = HashMap::new();
        params.insert("primaryCurrencyCode".into(), market.base_id.clone());
        params.insert("secondaryCurrencyCode".into(), market.quote_id.clone());

        let response: IndependentreserveTicker = self
            .public_get("GetMarketSummary", Some(params))
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market = self.markets.read().unwrap()
            .get(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?
            .clone();

        let mut params = HashMap::new();
        params.insert("primaryCurrencyCode".into(), market.base_id.clone());
        params.insert("secondaryCurrencyCode".into(), market.quote_id.clone());

        let response: IndependentreserveOrderBook = self
            .public_get("GetOrderBook", Some(params))
            .await?;

        let timestamp = response.created_timestamp_utc
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let mut bids: Vec<OrderBookEntry> = response
            .buy_orders
            .iter()
            .map(|o| OrderBookEntry {
                price: o.price.unwrap_or_default(),
                amount: o.volume.unwrap_or_default(),
            })
            .collect();

        let mut asks: Vec<OrderBookEntry> = response
            .sell_orders
            .iter()
            .map(|o| OrderBookEntry {
                price: o.price.unwrap_or_default(),
                amount: o.volume.unwrap_or_default(),
            })
            .collect();

        if let Some(l) = limit {
            bids.truncate(l as usize);
            asks.truncate(l as usize);
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: response.created_timestamp_utc,
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

        let market = self.markets.read().unwrap()
            .get(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?
            .clone();

        let mut params = HashMap::new();
        params.insert("primaryCurrencyCode".into(), market.base_id.clone());
        params.insert("secondaryCurrencyCode".into(), market.quote_id.clone());
        params.insert("numberOfRecentTradesToRetrieve".into(), limit.unwrap_or(50).min(50).to_string());

        let response: IndependentreserveTradesResponse = self
            .public_get("GetRecentTrades", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .trades
            .iter()
            .map(|t| self.parse_trade(t, Some(symbol)))
            .collect();

        Ok(trades)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: Vec<IndependentreserveBalance> = self
            .private_post("GetAccounts", params)
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
        self.load_markets(false).await?;

        let market = self.markets.read().unwrap()
            .get(symbol)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?
            .clone();

        let order_type_str = format!(
            "{}{}",
            match order_type {
                OrderType::Market => "Market",
                OrderType::Limit => "Limit",
                _ => return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {:?}", order_type),
                }),
            },
            match side {
                OrderSide::Buy => "Bid",
                OrderSide::Sell => "Offer",
            }
        );

        let mut params = HashMap::new();
        params.insert(
            "primaryCurrencyCode".into(),
            serde_json::Value::String(market.base_id.clone()),
        );
        params.insert(
            "secondaryCurrencyCode".into(),
            serde_json::Value::String(market.quote_id.clone()),
        );
        params.insert("orderType".into(), serde_json::Value::String(order_type_str));
        params.insert("volume".into(), serde_json::Value::String(amount.to_string()));

        let response: IndependentreserveOrder = if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), serde_json::Value::String(price_val.to_string()));
            self.private_post("PlaceLimitOrder", params).await?
        } else {
            self.private_post("PlaceMarketOrder", params).await?
        };

        Ok(self.parse_order(&response, Some(symbol)))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderGuid".into(), serde_json::Value::String(id.to_string()));

        let response: IndependentreserveOrder = self
            .private_post("CancelOrder", params)
            .await?;

        Ok(self.parse_order(&response, Some(symbol)))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderGuid".into(), serde_json::Value::String(id.to_string()));

        let response: IndependentreserveOrder = self
            .private_post("GetOrderDetails", params)
            .await?;

        Ok(self.parse_order(&response, Some(symbol)))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("pageIndex".into(), serde_json::Value::Number(1.into()));
        params.insert("pageSize".into(), serde_json::Value::Number(limit.unwrap_or(50).into()));

        if let Some(s) = symbol {
            self.load_markets(false).await?;
            let market = self.markets.read().unwrap()
                .get(s)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: s.to_string(),
                })?
                .clone();

            params.insert(
                "primaryCurrencyCode".into(),
                serde_json::Value::String(market.base_id.clone()),
            );
            params.insert(
                "secondaryCurrencyCode".into(),
                serde_json::Value::String(market.quote_id.clone()),
            );
        }

        let response: IndependentreserveOrdersResponse = self
            .private_post("GetOpenOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("pageIndex".into(), serde_json::Value::Number(1.into()));
        params.insert("pageSize".into(), serde_json::Value::Number(limit.unwrap_or(50).into()));

        if let Some(s) = symbol {
            self.load_markets(false).await?;
            let market = self.markets.read().unwrap()
                .get(s)
                .ok_or_else(|| CcxtError::BadSymbol {
                    symbol: s.to_string(),
                })?
                .clone();

            params.insert(
                "primaryCurrencyCode".into(),
                serde_json::Value::String(market.base_id.clone()),
            );
            params.insert(
                "secondaryCurrencyCode".into(),
                serde_json::Value::String(market.quote_id.clone()),
            );
        }

        let response: IndependentreserveOrdersResponse = self
            .private_post("GetClosedOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        params.insert("pageIndex".into(), serde_json::Value::Number(1.into()));
        params.insert("pageSize".into(), serde_json::Value::Number(limit.unwrap_or(50).into()));

        let response: IndependentreserveMyTradesResponse = self
            .private_post("GetTrades", params)
            .await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        Ok(trades)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("primaryCurrencyCode".into(), serde_json::Value::String(code.to_string()));

        let response: IndependentreserveDepositAddress = self
            .private_post("GetDigitalCurrencyDepositAddress", params)
            .await?;

        Ok(DepositAddress {
            currency: code.to_string(),
            address: response.deposit_address.clone().unwrap_or_default(),
            tag: response.tag.clone(),
            network: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        _network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let mut params = HashMap::new();
        params.insert("primaryCurrencyCode".into(), serde_json::Value::String(code.to_string()));
        params.insert("withdrawalAddress".into(), serde_json::Value::String(address.to_string()));
        params.insert("amount".into(), serde_json::Value::String(amount.to_string()));

        if let Some(t) = tag {
            params.insert("destinationTag".into(), serde_json::Value::String(t.to_string()));
        }

        let response: IndependentreserveWithdrawal = self
            .private_post("WithdrawDigitalCurrency", params)
            .await?;

        let timestamp = response.created_timestamp_utc
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        Ok(Transaction {
            id: response.transaction_guid.clone().unwrap_or_default(),
            timestamp,
            datetime: response.created_timestamp_utc.clone(),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: response.primary_currency_code.clone().unwrap_or_else(|| code.to_string()),
            network: None,
            amount: response.amount.clone().and_then(|a| a.total).unwrap_or_default(),
            status: TransactionStatus::Pending,
            address: response.destination.clone().and_then(|d| d.address),
            tag: response.destination.clone().and_then(|d| d.tag),
            txid: None,
            fee: response.amount.clone().and_then(|a| a.fee.map(|f| crate::types::Fee {
                cost: Some(f),
                currency: Some(code.to_string()),
                rate: None,
            })),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_trading_fees(&self) -> CcxtResult<HashMap<String, crate::types::TradingFee>> {
        let response: Vec<IndependentreserveBrokerageFee> = self
            .private_post("GetBrokerageFees", HashMap::new())
            .await?;

        let markets = self.markets.read().unwrap();
        let mut fees = HashMap::new();

        for fee_data in response {
            let code = fee_data.currency_code.unwrap_or_default().to_uppercase();
            let fee_rate = fee_data.fee.unwrap_or_default();

            // Apply this fee to all markets with this base currency
            for (symbol, market) in markets.iter() {
                if market.base == code {
                    fees.insert(
                        symbol.clone(),
                        crate::types::TradingFee::new(symbol, fee_rate, fee_rate),
                    );
                }
            }
        }

        Ok(fees)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // IndependentReserve doesn't support OHLCV - return error
        Err(CcxtError::NotSupported {
            feature: format!("fetch_ohlcv for {}", symbol),
        })
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
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let base_url = if api == "public" {
            Self::PUBLIC_URL
        } else {
            Self::PRIVATE_URL
        };

        let mut url = format!("{}/{}", base_url, path);

        if api == "public" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, query);
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

// === Independent Reserve API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveTicker {
    #[serde(default)]
    day_highest_price: Option<Decimal>,
    #[serde(default)]
    day_lowest_price: Option<Decimal>,
    #[serde(default)]
    day_avg_price: Option<Decimal>,
    #[serde(default)]
    day_volume_xbt: Option<Decimal>,
    #[serde(default)]
    day_volume_xbt_in_secondary_currrency: Option<Decimal>,
    #[serde(default)]
    current_lowest_offer_price: Option<Decimal>,
    #[serde(default)]
    current_highest_bid_price: Option<Decimal>,
    #[serde(default)]
    last_price: Option<Decimal>,
    #[serde(default)]
    primary_currency_code: Option<String>,
    #[serde(default)]
    secondary_currency_code: Option<String>,
    #[serde(default)]
    created_timestamp_utc: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveOrderBook {
    #[serde(default)]
    created_timestamp_utc: Option<String>,
    #[serde(default)]
    buy_orders: Vec<IndependentreserveOrderBookEntry>,
    #[serde(default)]
    sell_orders: Vec<IndependentreserveOrderBookEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveOrderBookEntry {
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveTradesResponse {
    #[serde(default)]
    trades: Vec<IndependentreserveTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveTrade {
    #[serde(default)]
    trade_guid: Option<String>,
    #[serde(default)]
    order_guid: Option<String>,
    #[serde(default)]
    trade_timestamp_utc: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    volume_traded: Option<Decimal>,
    #[serde(default)]
    primary_currency_code: Option<String>,
    #[serde(default)]
    secondary_currency_code: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    secondary_currency_trade_price: Option<Decimal>,
    #[serde(default)]
    primary_currency_amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveOrder {
    #[serde(default)]
    order_guid: Option<String>,
    #[serde(default)]
    created_timestamp_utc: Option<String>,
    #[serde(default)]
    #[serde(rename = "Type")]
    type_field: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    volume_ordered: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    volume_filled: Option<Decimal>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    avg_price: Option<Decimal>,
    #[serde(default)]
    reserved_amount: Option<Decimal>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    primary_currency_code: Option<String>,
    #[serde(default)]
    secondary_currency_code: Option<String>,
    #[serde(default)]
    outstanding: Option<Decimal>,
    #[serde(default)]
    value: Option<Decimal>,
    #[serde(default)]
    fee_percent: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveOrdersResponse {
    #[serde(default)]
    data: Vec<IndependentreserveOrder>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveMyTradesResponse {
    #[serde(default)]
    data: Vec<IndependentreserveTrade>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveBalance {
    #[serde(default)]
    currency_code: Option<String>,
    #[serde(default)]
    available_balance: Option<Decimal>,
    #[serde(default)]
    total_balance: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveDepositAddress {
    #[serde(default)]
    deposit_address: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    last_checked_timestamp_utc: Option<String>,
    #[serde(default)]
    next_update_timestamp_utc: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveWithdrawal {
    #[serde(default)]
    transaction_guid: Option<String>,
    #[serde(default)]
    primary_currency_code: Option<String>,
    #[serde(default)]
    created_timestamp_utc: Option<String>,
    #[serde(default)]
    amount: Option<IndependentreserveAmount>,
    #[serde(default)]
    destination: Option<IndependentreserveDestination>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveAmount {
    #[serde(default)]
    total: Option<Decimal>,
    #[serde(default)]
    fee: Option<Decimal>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveDestination {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    tag: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct IndependentreserveBrokerageFee {
    #[serde(default)]
    currency_code: Option<String>,
    #[serde(default)]
    fee: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Independentreserve::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Independentreserve);
        assert_eq!(exchange.name(), "Independent Reserve");
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
    }
}
