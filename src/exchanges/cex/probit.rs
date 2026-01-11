//! ProBit Exchange Implementation
//!
//! Korean cryptocurrency exchange

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// ProBit 거래소
pub struct ProBit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    access_token: RwLock<Option<String>>,
}

impl ProBit {
    const BASE_URL: &'static str = "https://api.probit.com/api/exchange/v1";
    const AUTH_URL: &'static str = "https://accounts.probit.com";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// 새 ProBit 인스턴스 생성
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
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/79268032-c4379480-7ea2-11ea-80b3-dd96bb29fd0d.jpg".into()),
            api: api_urls,
            www: Some("https://www.probit.com".into()),
            doc: vec![
                "https://docs-en.probit.com".into(),
                "https://docs-en.probit.com/docs/authorization-1".into(),
            ],
            fees: Some("https://www.probit.com/en-us/fee".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
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
            access_token: RwLock::new(None),
        })
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// OAuth 인증 토큰 가져오기
    async fn authenticate(&self) -> CcxtResult<String> {
        // 캐시된 토큰이 있으면 반환
        {
            let token = self.access_token.read().unwrap();
            if let Some(t) = token.as_ref() {
                return Ok(t.clone());
            }
        }

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        // OAuth 토큰 요청
        let auth_client = HttpClient::new(Self::AUTH_URL, &self.config)?;

        let credentials = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{api_key}:{api_secret}").as_bytes(),
        );

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Basic {credentials}"));
        headers.insert("Content-Type".into(), "application/json".into());

        let body = serde_json::json!({
            "grant_type": "client_credentials"
        });

        let response: ProBitAuthResponse = auth_client
            .post("/token", Some(body), Some(headers))
            .await?;

        let token = response.access_token.ok_or_else(|| CcxtError::AuthenticationError {
            message: "Failed to get access token".into(),
        })?;

        // 토큰 캐시 저장
        {
            let mut stored = self.access_token.write().unwrap();
            *stored = Some(token.clone());
        }

        Ok(token)
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let token = self.authenticate().await?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {token}"));
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => self.client.post(path, body, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 ID 변환 (BTC/USDT -> BTC-USDT)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 마켓 ID를 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn convert_from_market_id(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }
}

#[async_trait]
impl Exchange for ProBit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Probit
    }

    fn name(&self) -> &str {
        "ProBit"
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
        let response: ProBitResponse<Vec<ProBitMarket>> = self.public_get("/market", None).await?;

        let data = response.data.unwrap_or_default();
        let mut markets = Vec::new();

        for market_data in data {
            let id = market_data.id.clone().unwrap_or_default();
            let base = market_data.base_currency_id.clone().unwrap_or_default();
            let quote = market_data.quote_currency_id.clone().unwrap_or_default();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active: !market_data.closed.unwrap_or(false),
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: market_data.taker_fee_rate.as_ref().and_then(|s| s.parse().ok()),
                maker: market_data.maker_fee_rate.as_ref().and_then(|s| s.parse().ok()),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: market_data.quantity_precision,
                    price: market_data.price_precision,
                    cost: market_data.cost_precision,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: market_data.min_quantity.as_ref().and_then(|s| s.parse().ok()),
                        max: market_data.max_quantity.as_ref().and_then(|s| s.parse().ok()),
                    },
                    price: crate::types::MinMax {
                        min: market_data.min_price.as_ref().and_then(|s| s.parse().ok()),
                        max: market_data.max_price.as_ref().and_then(|s| s.parse().ok()),
                    },
                    cost: crate::types::MinMax {
                        min: market_data.min_cost.as_ref().and_then(|s| s.parse().ok()),
                        max: market_data.max_cost.as_ref().and_then(|s| s.parse().ok()),
                    },
                    leverage: crate::types::MinMax::default(),
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_data).unwrap_or_default(),
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
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market_ids".into(), market_id);

        let response: ProBitResponse<Vec<ProBitTicker>> =
            self.public_get("/ticker", Some(params)).await?;

        let data = response.data.unwrap_or_default();
        if let Some(ticker_data) = data.first() {
            let timestamp = ticker_data
                .time
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| Utc::now().timestamp_millis());

            Ok(Ticker {
                symbol: symbol.to_string(),
                timestamp: Some(timestamp),
                datetime: ticker_data.time.clone(),
                high: ticker_data.high.as_ref().and_then(|s| s.parse().ok()),
                low: ticker_data.low.as_ref().and_then(|s| s.parse().ok()),
                bid: ticker_data.bid.as_ref().and_then(|s| s.parse().ok()),
                bid_volume: None,
                ask: ticker_data.ask.as_ref().and_then(|s| s.parse().ok()),
                ask_volume: None,
                vwap: None,
                open: ticker_data.open.as_ref().and_then(|s| s.parse().ok()),
                close: ticker_data.last.as_ref().and_then(|s| s.parse().ok()),
                last: ticker_data.last.as_ref().and_then(|s| s.parse().ok()),
                previous_close: None,
                change: ticker_data.change.as_ref().and_then(|s| s.parse().ok()),
                percentage: ticker_data
                    .change_percent
                    .as_ref()
                    .and_then(|s| s.parse().ok()),
                average: None,
                base_volume: ticker_data
                    .base_volume
                    .as_ref()
                    .and_then(|s| s.parse().ok()),
                quote_volume: ticker_data
                    .quote_volume
                    .as_ref()
                    .and_then(|s| s.parse().ok()),
                index_price: None,
                mark_price: None,
                info: serde_json::to_value(ticker_data).unwrap_or_default(),
            })
        } else {
            Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        }
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market_id".into(), market_id);

        let response: ProBitResponse<ProBitOrderBook> =
            self.public_get("/order_book", Some(params)).await?;

        let data = response.data.ok_or_else(|| CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

        let parse_entries =
            |entries: &Vec<ProBitOrderBookLevel>| -> Vec<OrderBookEntry> {
                let iter = entries.iter().filter_map(|e| {
                    Some(OrderBookEntry {
                        price: e.price.as_ref()?.parse().ok()?,
                        amount: e.quantity.as_ref()?.parse().ok()?,
                    })
                });
                if let Some(l) = limit {
                    iter.take(l as usize).collect()
                } else {
                    iter.collect()
                }
            };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids: parse_entries(&data.bids),
            asks: parse_entries(&data.asks),
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market_id".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        if let Some(s) = since {
            if let Some(dt) = chrono::DateTime::from_timestamp_millis(s) {
                params.insert("start_time".into(), dt.to_rfc3339());
            }
        }

        let response: ProBitResponse<Vec<ProBitTrade>> =
            self.public_get("/trade", Some(params)).await?;

        let data = response.data.unwrap_or_default();
        let trades: Vec<Trade> = data
            .iter()
            .map(|t| {
                let timestamp = t
                    .time
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let price: Decimal = t
                    .price
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let amount: Decimal = t
                    .quantity
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                Trade {
                    id: t.id.clone().unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: t.time.clone(),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.side.clone(),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
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
        let market_id = self.convert_to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1h".into());

        let mut params = HashMap::new();
        params.insert("market_ids".into(), market_id);
        params.insert("interval".into(), interval);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        if let Some(s) = since {
            if let Some(dt) = chrono::DateTime::from_timestamp_millis(s) {
                params.insert("start_time".into(), dt.to_rfc3339());
            }
        }

        let response: ProBitResponse<Vec<ProBitCandle>> =
            self.public_get("/candle", Some(params)).await?;

        let data = response.data.unwrap_or_default();
        let ohlcv: Vec<OHLCV> = data
            .iter()
            .filter_map(|c| {
                let timestamp = c
                    .start_time
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())?;
                Some(OHLCV {
                    timestamp,
                    open: c.open.as_ref()?.parse().ok()?,
                    high: c.high.as_ref()?.parse().ok()?,
                    low: c.low.as_ref()?.parse().ok()?,
                    close: c.close.as_ref()?.parse().ok()?,
                    volume: c.volume.as_ref()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: ProBitResponse<Vec<ProBitBalance>> =
            self.private_request("GET", "/balance", None).await?;

        let data = response.data.unwrap_or_default();

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for balance_data in data {
            if let Some(currency) = balance_data.currency_id {
                let free: Decimal = balance_data
                    .available
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();
                let total: Decimal = balance_data
                    .total
                    .as_ref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default();

                balances.currencies.insert(
                    currency.to_uppercase(),
                    Balance {
                        free: Some(free),
                        used: Some(total - free),
                        total: Some(total),
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
        let market_id = self.convert_to_market_id(symbol);

        let mut body = serde_json::json!({
            "market_id": market_id,
            "side": match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            },
            "type": match order_type {
                OrderType::Limit => "limit",
                OrderType::Market => "market",
                _ => "limit",
            },
            "quantity": amount.to_string(),
            "time_in_force": "gtc",
        });

        if order_type == OrderType::Limit {
            let p = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price is required for limit orders".into(),
            })?;
            body["limit_price"] = serde_json::json!(p.to_string());
        }

        let response: ProBitResponse<ProBitOrder> = self
            .private_request("POST", "/new_order", Some(body))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to create order".into(),
        })?;

        let timestamp = data
            .time
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.time.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None, // ProBit uses string time_in_force
            side,
            price,
            average: None,
            amount,
            filled: data
                .filled_quantity
                .as_ref()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            remaining: data.open_quantity.as_ref().and_then(|s| s.parse().ok()),
            cost: data.filled_cost.as_ref().and_then(|s| s.parse().ok()),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let body = serde_json::json!({
            "market_id": market_id,
            "order_id": id,
        });

        let response: ProBitResponse<ProBitOrder> = self
            .private_request("POST", "/cancel_order", Some(body))
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        let timestamp = data
            .time
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("cancelled") => OrderStatus::Canceled,
            _ => OrderStatus::Canceled,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        Ok(Order {
            id: id.to_string(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: data.time.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None, // ProBit uses string time_in_force
            side,
            price: data.limit_price.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            amount: data
                .quantity
                .as_ref()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            filled: data
                .filled_quantity
                .as_ref()
                .and_then(|s| s.parse().ok())
                .unwrap_or_default(),
            remaining: data.open_quantity.as_ref().and_then(|s| s.parse().ok()),
            cost: data.filled_cost.as_ref().and_then(|s| s.parse().ok()),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // ProBit은 직접적인 fetch order API가 없으므로 open orders에서 찾음
        let orders = self.fetch_open_orders(Some(symbol), None, None).await?;

        orders
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params: HashMap<String, String> = HashMap::new();
        if let Some(s) = symbol {
            params.insert("market_id".into(), self.convert_to_market_id(s));
        }

        let response: ProBitResponse<Vec<ProBitOrder>> = self
            .private_request("GET", "/open_order", None)
            .await?;

        let data = response.data.unwrap_or_default();
        let orders: Vec<Order> = data
            .iter()
            .map(|o| {
                let timestamp = o
                    .time
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let status = match o.status.as_deref() {
                    Some("open") => OrderStatus::Open,
                    Some("filled") => OrderStatus::Closed,
                    Some("cancelled") => OrderStatus::Canceled,
                    _ => OrderStatus::Open,
                };

                let side = match o.side.as_deref() {
                    Some("buy") => OrderSide::Buy,
                    Some("sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some("limit") => OrderType::Limit,
                    Some("market") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                let symbol_id = o.market_id.clone().unwrap_or_default();
                let sym = self
                    .symbol(&symbol_id)
                    .unwrap_or_else(|| self.convert_from_market_id(&symbol_id));

                Order {
                    id: o.id.clone().unwrap_or_default(),
                    client_order_id: o.client_order_id.clone(),
                    timestamp: Some(timestamp),
                    datetime: o.time.clone(),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status,
                    symbol: sym,
                    order_type,
                    time_in_force: None, // ProBit uses string time_in_force
                    side,
                    price: o.limit_price.as_ref().and_then(|s| s.parse().ok()),
                    average: o
                        .filled_cost
                        .as_ref()
                        .zip(o.filled_quantity.as_ref())
                        .and_then(|(cost, qty)| {
                            let c: Decimal = cost.parse().ok()?;
                            let q: Decimal = qty.parse().ok()?;
                            if q > Decimal::ZERO {
                                Some(c / q)
                            } else {
                                None
                            }
                        }),
                    amount: o
                        .quantity
                        .as_ref()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    filled: o
                        .filled_quantity
                        .as_ref()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default(),
                    remaining: o.open_quantity.as_ref().and_then(|s| s.parse().ok()),
                    cost: o.filled_cost.as_ref().and_then(|s| s.parse().ok()),
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
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

#[derive(Debug, Deserialize)]
struct ProBitResponse<T> {
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ProBitAuthResponse {
    #[serde(default)]
    access_token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProBitMarket {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    base_currency_id: Option<String>,
    #[serde(default)]
    quote_currency_id: Option<String>,
    #[serde(default)]
    quantity_precision: Option<i32>,
    #[serde(default)]
    price_precision: Option<i32>,
    #[serde(default)]
    cost_precision: Option<i32>,
    #[serde(default)]
    min_quantity: Option<String>,
    #[serde(default)]
    max_quantity: Option<String>,
    #[serde(default)]
    min_price: Option<String>,
    #[serde(default)]
    max_price: Option<String>,
    #[serde(default)]
    min_cost: Option<String>,
    #[serde(default)]
    max_cost: Option<String>,
    #[serde(default)]
    taker_fee_rate: Option<String>,
    #[serde(default)]
    maker_fee_rate: Option<String>,
    #[serde(default)]
    closed: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProBitTicker {
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    base_volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default)]
    change_percent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProBitOrderBook {
    #[serde(default)]
    bids: Vec<ProBitOrderBookLevel>,
    #[serde(default)]
    asks: Vec<ProBitOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct ProBitOrderBookLevel {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProBitTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProBitCandle {
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ProBitOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    market_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    filled_quantity: Option<String>,
    #[serde(default)]
    open_quantity: Option<String>,
    #[serde(default)]
    filled_cost: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProBitBalance {
    #[serde(default)]
    currency_id: Option<String>,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    total: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = ProBit::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Probit);
        assert_eq!(exchange.name(), "ProBit");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = ProBit::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USDT"), "BTC-USDT");
        assert_eq!(exchange.convert_from_market_id("BTC-USDT"), "BTC/USDT");
    }
}
