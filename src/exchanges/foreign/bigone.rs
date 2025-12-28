//! BigONE Exchange Implementation
//!
//! CCXT bigone.ts를 Rust로 포팅

#![allow(dead_code)]

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
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, TimeInForce, Trade,
    OHLCV,
};

/// BigONE 거래소
pub struct Bigone {
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

impl Bigone {
    const BASE_URL: &'static str = "https://big.one/api/v3";
    const RATE_LIMIT_MS: u64 = 20; // 500 requests per 10 seconds

    /// 새 BigONE 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: true,
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
            fetch_deposit_address: true,
            withdraw: true,
            fetch_time: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), "https://big.one/api/v3".into());
        api_urls.insert("private".into(), "https://big.one/api/v3/viewer".into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/4e5cfd53-98cc-4b90-92cd-0d7b512653d1".into()),
            api: api_urls,
            www: Some("https://big.one".into()),
            doc: vec![
                "https://open.big.one/docs/api.html".into(),
            ],
            fees: Some("https://bigone.zendesk.com/hc/en-us/articles/115001933374-BigONE-Fee-Policy".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "min1".into());
        timeframes.insert(Timeframe::Minute5, "min5".into());
        timeframes.insert(Timeframe::Minute15, "min15".into());
        timeframes.insert(Timeframe::Minute30, "min30".into());
        timeframes.insert(Timeframe::Hour1, "hour1".into());
        timeframes.insert(Timeframe::Hour3, "hour3".into());
        timeframes.insert(Timeframe::Hour4, "hour4".into());
        timeframes.insert(Timeframe::Hour6, "hour6".into());
        timeframes.insert(Timeframe::Hour12, "hour12".into());
        timeframes.insert(Timeframe::Day1, "day1".into());
        timeframes.insert(Timeframe::Week1, "week1".into());
        timeframes.insert(Timeframe::Month1, "month1".into());

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
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        self.public_client.get(&url, None, None).await
    }

    /// 비공개 API 호출 (JWT 인증 사용)
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        // BigONE uses JWT authentication
        let _nonce = Utc::now().timestamp_millis().to_string();

        // Create JWT token (simplified - actual implementation would need proper JWT library)
        let jwt_token = format!("{}.{}", api_key, secret); // Placeholder

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {}", jwt_token));
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => {
                let url = if params.is_empty() {
                    path.to_string()
                } else {
                    let query: String = params
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                        .collect::<Vec<_>>()
                        .join("&");
                    format!("{path}?{query}")
                };
                self.private_client.get(&url, None, Some(headers)).await
            }
            "POST" => {
                // Convert params HashMap to JSON body
                let body = if params.is_empty() {
                    None
                } else {
                    Some(serde_json::to_value(&params).unwrap_or_default())
                };
                self.private_client.post(path, body, Some(headers)).await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTC-USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 마켓 ID → 심볼 변환 (BTC-USDT → BTC/USDT)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BigoneTicker, symbol: &str) -> Ticker {
        let close = data.close.or(data.latest_price);

        Ticker {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            high: data.high.or(data.last_24h_max_price),
            low: data.low.or(data.last_24h_min_price),
            bid: data.bid.as_ref().and_then(|b| b.price),
            bid_volume: data.bid.as_ref().and_then(|b| b.quantity),
            ask: data.ask.as_ref().and_then(|b| b.price),
            ask_volume: data.ask.as_ref().and_then(|b| b.quantity),
            vwap: None,
            open: data.open,
            close,
            last: close,
            previous_close: None,
            change: data.daily_change.or(data.last_24h_price_change),
            percentage: None,
            average: None,
            base_volume: data.volume.or(data.volume_24h),
            quote_volume: data.volume_24h_in_usd,
            index_price: data.index_price,
            mark_price: data.mark_price,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BigoneOrder, symbol: &str) -> Order {
        let status = match data.state.as_str() {
            "PENDING" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELLED" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP_LIMIT" => OrderType::StopLossLimit,
            "STOP_MARKET" => OrderType::StopLoss,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "BID" => OrderSide::Buy,
            "ASK" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = if data.immediate_or_cancel.unwrap_or(false) {
            Some(TimeInForce::IOC)
        } else {
            None
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.amount.parse().unwrap_or_default();
        let filled: Decimal = data.filled_amount.parse().unwrap_or_default();
        let remaining = Some(amount - filled);
        let average = data.avg_deal_price.as_ref().and_then(|p| p.parse().ok());
        let cost = average.and_then(|avg| Some(avg * filled));

        let stop_price = data.stop_price.as_ref()
            .and_then(|p| p.parse::<Decimal>().ok())
            .filter(|p| *p != Decimal::ZERO);

        Order {
            id: data.id.to_string(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.created_at.as_ref().map(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_default()
            }),
            datetime: data.created_at.clone(),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_at.as_ref().map(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_default()
            }),
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price,
            trigger_price: stop_price,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: data.post_only,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[BigoneBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.balance.parse().ok();
            let used: Option<Decimal> = b.locked_balance.parse().ok();
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
            result.add(&b.asset_symbol, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Bigone {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bigone
    }

    fn name(&self) -> &str {
        "BigONE"
    }

    fn version(&self) -> &str {
        "v3"
    }

    fn countries(&self) -> &[&str] {
        &["CN"]
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
        let response: BigoneMarketsResponse = self
            .public_get("/asset_pairs", None)
            .await?;

        let mut markets = Vec::new();

        for market_data in response.data {
            let base_asset = market_data.base_asset.clone();
            let quote_asset = market_data.quote_asset.clone();
            let base = base_asset.symbol.clone();
            let quote = quote_asset.symbol.clone();
            let symbol = format!("{}/{}", base, quote);

            let market = Market {
                id: market_data.name.clone(),
                lowercase_id: Some(market_data.name.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base_asset.symbol.clone(),
                quote_id: quote_asset.symbol.clone(),
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
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::new(1, 3)), // 0.1%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(market_data.base_scale),
                    price: Some(market_data.quote_scale),
                    cost: None,
                    base: Some(market_data.base_scale),
                    quote: Some(market_data.quote_scale),
                },
                limits: MarketLimits {
                    leverage: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    amount: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    price: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    cost: crate::types::MinMax {
                        min: market_data.min_quote_value.parse().ok(),
                        max: market_data.max_quote_value.parse().ok(),
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

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/asset_pairs/{}/ticker", market_id);

        let response: BigoneTickerResponse = self
            .public_get(&path, None)
            .await?;

        Ok(self.parse_ticker(&response.data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let mut params = HashMap::new();
        if let Some(syms) = symbols {
            let ids: Vec<String> = syms.iter().map(|s| self.to_market_id(s)).collect();
            params.insert("pair_names".into(), ids.join(","));
        }

        let response: BigoneTickersResponse = self
            .public_get("/asset_pairs/tickers", Some(params))
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response.data {
            if let Some(market_id) = data.asset_pair_name.as_ref() {
                if let Some(symbol) = markets_by_id.get(market_id) {
                    if let Some(filter) = symbols {
                        if !filter.contains(&symbol.as_str()) {
                            continue;
                        }
                    }

                    let ticker = self.parse_ticker(&data, symbol);
                    tickers.insert(symbol.clone(), ticker);
                }
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let path = format!("/asset_pairs/{}/depth", market_id);
        let response: BigoneOrderBookResponse = self
            .public_get(&path, Some(params))
            .await?;

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

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let path = format!("/asset_pairs/{}/trades", market_id);
        let response: BigoneTradesResponse = self
            .public_get(&path, Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&t.created_at)
                    .map(|dt| dt.timestamp_millis())
                    .ok();

                let side = if t.taker_side == "ASK" {
                    Some("sell".into())
                } else {
                    Some("buy".into())
                };

                Trade {
                    id: t.id.to_string(),
                    order: None,
                    timestamp,
                    datetime: Some(t.created_at.clone()),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side,
                    taker_or_maker: None,
                    price: t.price.parse().unwrap_or_default(),
                    amount: t.amount.parse().unwrap_or_default(),
                    cost: Some(
                        t.price.parse::<Decimal>().unwrap_or_default()
                            * t.amount.parse::<Decimal>().unwrap_or_default(),
                    ),
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
        let market_id = self.to_market_id(symbol);
        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {:?}", timeframe),
        })?;

        let mut params = HashMap::new();
        params.insert("period".into(), interval.clone());

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        if let Some(s) = since {
            let datetime = chrono::DateTime::<Utc>::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("time".into(), datetime);
        }

        let path = format!("/asset_pairs/{}/candles", market_id);
        let response: BigoneOHLCVResponse = self
            .public_get(&path, Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .data
            .iter()
            .filter_map(|c| {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&c.time)
                    .map(|dt| dt.timestamp_millis())
                    .ok()?;

                Some(OHLCV {
                    timestamp,
                    open: c.open.parse().ok()?,
                    high: c.high.parse().ok()?,
                    low: c.low.parse().ok()?,
                    close: c.close.parse().ok()?,
                    volume: c.volume.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: BigoneBalanceResponse = self
            .private_request("GET", "/viewer/accounts", params)
            .await?;

        Ok(self.parse_balance(&response.data))
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
        params.insert("asset_pair_name".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "BID",
            OrderSide::Sell => "ASK",
        }.into());
        params.insert("type".into(), match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {:?}", order_type),
            }),
        }.into());
        params.insert("amount".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: BigoneOrderResponse = self
            .private_request("POST", "/viewer/orders", params)
            .await?;

        Ok(self.parse_order(&response.data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let params = HashMap::new();
        let path = format!("/viewer/orders/{}/cancel", id);

        let response: BigoneOrderResponse = self
            .private_request("POST", &path, params)
            .await?;

        Ok(self.parse_order(&response.data, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let params = HashMap::new();
        let path = format!("/viewer/orders/{}", id);

        let response: BigoneOrderResponse = self
            .private_request("GET", &path, params)
            .await?;

        Ok(self.parse_order(&response.data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "BigONE fetchOpenOrders requires a symbol argument".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("asset_pair_name".into(), market_id);
        params.insert("state".into(), "PENDING".into());

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BigoneOrdersResponse = self
            .private_request("GET", "/viewer/orders", params)
            .await?;

        let orders: Vec<Order> = response
            .data
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_time(&self) -> CcxtResult<i64> {
        let response: BigonePingResponse = self
            .public_get("/ping", None)
            .await?;

        // BigONE returns timestamp in nanoseconds
        Ok(response.data.timestamp / 1_000_000)
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "BigONE cancelAllOrders requires a symbol argument".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("asset_pair_name".into(), market_id);

        let response: BigoneCancelAllResponse = self
            .private_request("POST", "/viewer/orders/cancel", params)
            .await?;

        let mut orders = Vec::new();

        for order_id in response.data.cancelled {
            orders.push(Order {
                id: order_id.to_string(),
                status: OrderStatus::Canceled,
                symbol: symbol.to_string(),
                ..Default::default()
            });
        }

        Ok(orders)
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
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        // BigONE uses JWT authentication which is handled in private_request
        SignedRequest {
            url: format!("{}{}", Self::BASE_URL, path),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "BigONE fetchMyTrades requires a symbol argument".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("asset_pair_name".into(), market_id);

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BigoneMyTradesResponse = self
            .private_request("GET", "/viewer/trades", params)
            .await?;

        let trades: Vec<Trade> = response
            .data
            .iter()
            .map(|t| {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&t.inserted_at)
                    .map(|dt| dt.timestamp_millis())
                    .ok();

                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.amount.parse().unwrap_or_default();
                let cost = price * amount;

                let side = match t.side.as_str() {
                    "BID" => Some("buy".into()),
                    "ASK" => Some("sell".into()),
                    _ => None,
                };

                let taker_or_maker = if t.taker_side.as_ref() == Some(&t.side) {
                    Some(TakerOrMaker::Taker)
                } else {
                    Some(TakerOrMaker::Maker)
                };

                Trade {
                    id: t.id.to_string(),
                    order: t.taker_order_id.map(|id| id.to_string())
                        .or_else(|| t.maker_order_id.map(|id| id.to_string())),
                    timestamp,
                    datetime: Some(t.inserted_at.clone()),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side,
                    taker_or_maker,
                    price,
                    amount,
                    cost: Some(cost),
                    fee: t.taker_fee.as_ref().or(t.maker_fee.as_ref()).and_then(|f| {
                        f.parse::<Decimal>().ok().map(|cost| crate::types::Fee {
                            cost: Some(cost),
                            currency: None,
                            rate: None,
                        })
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "BigONE fetchClosedOrders requires a symbol argument".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("asset_pair_name".into(), market_id);
        params.insert("state".into(), "FILLED".into());

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BigoneOrdersResponse = self
            .private_request("GET", "/viewer/orders", params)
            .await?;

        let orders: Vec<Order> = response
            .data
            .iter()
            .filter(|o| o.state == "FILLED" || o.state == "CANCELLED")
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }
}

// === BigONE API Response Types ===

#[derive(Debug, Deserialize)]
struct BigoneMarketsResponse {
    data: Vec<BigoneMarket>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneMarket {
    id: String,
    name: String,
    base_asset: BigoneAsset,
    quote_asset: BigoneAsset,
    base_scale: i32,
    quote_scale: i32,
    min_quote_value: String,
    max_quote_value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BigoneAsset {
    id: String,
    symbol: String,
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneTicker {
    #[serde(default)]
    asset_pair_name: Option<String>,
    #[serde(default)]
    bid: Option<BigoneBidAsk>,
    #[serde(default)]
    ask: Option<BigoneBidAsk>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    daily_change: Option<Decimal>,
    // Swap/Contract fields
    #[serde(default)]
    latest_price: Option<Decimal>,
    #[serde(default)]
    last_24h_price_change: Option<Decimal>,
    #[serde(default)]
    last_24h_max_price: Option<Decimal>,
    #[serde(default)]
    last_24h_min_price: Option<Decimal>,
    #[serde(default)]
    volume_24h: Option<Decimal>,
    #[serde(default)]
    volume_24h_in_usd: Option<Decimal>,
    #[serde(default)]
    mark_price: Option<Decimal>,
    #[serde(default)]
    index_price: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneBidAsk {
    price: Option<Decimal>,
    quantity: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BigoneTickerResponse {
    data: BigoneTicker,
}

#[derive(Debug, Deserialize)]
struct BigoneTickersResponse {
    data: Vec<BigoneTicker>,
}

#[derive(Debug, Deserialize)]
struct BigoneOrderBookResponse {
    data: BigoneOrderBookData,
}

#[derive(Debug, Deserialize)]
struct BigoneOrderBookData {
    bids: Vec<BigoneOrderBookLevel>,
    asks: Vec<BigoneOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct BigoneOrderBookLevel {
    price: String,
    quantity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneTrade {
    id: i64,
    price: String,
    amount: String,
    taker_side: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct BigoneTradesResponse {
    data: Vec<BigoneTrade>,
}

#[derive(Debug, Deserialize)]
struct BigoneOHLCVResponse {
    data: Vec<BigoneOHLCV>,
}

#[derive(Debug, Deserialize)]
struct BigoneOHLCV {
    time: String,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneOrder {
    id: i64,
    #[serde(default)]
    client_order_id: Option<String>,
    asset_pair_name: String,
    price: Option<String>,
    amount: String,
    filled_amount: String,
    #[serde(default)]
    avg_deal_price: Option<String>,
    side: String,
    state: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(rename = "type")]
    order_type: String,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    immediate_or_cancel: Option<bool>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct BigoneOrderResponse {
    data: BigoneOrder,
}

#[derive(Debug, Deserialize)]
struct BigoneOrdersResponse {
    data: Vec<BigoneOrder>,
}

#[derive(Debug, Deserialize)]
struct BigoneBalanceResponse {
    data: Vec<BigoneBalance>,
}

#[derive(Debug, Deserialize)]
struct BigoneBalance {
    asset_symbol: String,
    balance: String,
    locked_balance: String,
}

#[derive(Debug, Deserialize)]
struct BigonePingResponse {
    data: BigonePingData,
}

#[derive(Debug, Deserialize)]
struct BigonePingData {
    #[serde(rename = "timestamp")]
    timestamp: i64,
}

#[derive(Debug, Deserialize)]
struct BigoneCancelAllResponse {
    data: BigoneCancelAllData,
}

#[derive(Debug, Deserialize)]
struct BigoneCancelAllData {
    cancelled: Vec<i64>,
    failed: Vec<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BigoneMyTrade {
    id: i64,
    asset_pair_name: String,
    price: String,
    amount: String,
    #[serde(default)]
    taker_side: Option<String>,
    #[serde(default)]
    maker_order_id: Option<i64>,
    #[serde(default)]
    taker_order_id: Option<i64>,
    #[serde(default)]
    maker_fee: Option<String>,
    #[serde(default)]
    taker_fee: Option<String>,
    side: String,
    inserted_at: String,
}

#[derive(Debug, Deserialize)]
struct BigoneMyTradesResponse {
    data: Vec<BigoneMyTrade>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let bigone = Bigone::new(config).unwrap();

        assert_eq!(bigone.to_market_id("BTC/USDT"), "BTC-USDT");
        assert_eq!(bigone.to_symbol("BTC-USDT"), "BTC/USDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let bigone = Bigone::new(config).unwrap();

        assert_eq!(bigone.id(), ExchangeId::Bigone);
        assert_eq!(bigone.name(), "BigONE");
        assert!(bigone.has().spot);
        assert!(bigone.has().swap);
    }
}
