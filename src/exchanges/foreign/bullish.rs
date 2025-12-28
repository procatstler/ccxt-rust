//! Bullish Exchange Implementation
//!
//! CCXT bullish.ts를 Rust로 포팅

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, TimeInForce, Trade,
    OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Bullish 거래소
pub struct Bullish {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    trading_account_id: RwLock<Option<String>>,
}

impl Bullish {
    const BASE_URL: &'static str = "https://api.exchange.bullish.com/trading-api";
    const RATE_LIMIT_MS: u64 = 20; // 50 requests per second

    /// 새 Bullish 인스턴스 생성
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
            fetch_ticker: true,
            fetch_tickers: false,
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
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/68f0686b-84f0-4da9-a751-f7089af3a9ed".into()),
            api: api_urls,
            www: Some("https://bullish.com/".into()),
            doc: vec![
                "https://api.exchange.bullish.com/docs/api/rest/".into(),
            ],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());

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
            trading_account_id: RwLock::new(None),
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

    /// 비공개 API 호출 (with trading account)
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        mut params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        // Add trading account ID if available
        if let Some(account_id) = self.trading_account_id.read().unwrap().as_ref() {
            params.insert("tradingAccountId".into(), account_id.clone());
        }

        let timestamp = Utc::now().timestamp_millis().to_string();
        params.insert("timestamp".into(), timestamp);

        let query: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Create HMAC-SHA256 signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let signed_query = format!("{query}&signature={signature}");

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {}", api_key));
        headers.insert("Content-Type".into(), "application/json".into());

        let url = format!("{path}?{signed_query}");

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => self.private_client.post(&url, None, Some(headers)).await,
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDC → BTCUSDC)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BullishTicker, symbol: &str) -> Ticker {
        let timestamp = data.created_at_timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.best_bid,
            bid_volume: data.bid_volume,
            ask: data.best_ask,
            ask_volume: data.ask_volume,
            vwap: data.vwap,
            open: data.open,
            close: data.close,
            last: data.last,
            previous_close: None,
            change: data.change,
            percentage: data.percentage,
            average: data.average,
            base_volume: data.base_volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: data.mark_price,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BullishOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            Some("OPEN") | Some("NEW") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELED") | Some("CANCELLED") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            Some("EXPIRED") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") | Some("LMT") => OrderType::Limit,
            Some("MARKET") | Some("MKT") => OrderType::Market,
            Some("STOP_LIMIT") => OrderType::StopLossLimit,
            Some("POST_ONLY") => OrderType::LimitMaker,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data.time_in_force.as_ref().and_then(|tif| match tif.as_str() {
            "GTC" => Some(TimeInForce::GTC),
            "IOC" => Some(TimeInForce::IOC),
            "FOK" => Some(TimeInForce::FOK),
            _ => None,
        });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.quantity.as_ref().and_then(|q| q.parse().ok()).unwrap_or_default();
        let filled: Decimal = data.filled_quantity.as_ref().and_then(|f| f.parse().ok()).unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data.cumulative_cost.as_ref().and_then(|c| c.parse().ok());
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.created_at_timestamp,
            datetime: data.created_at_timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: data.updated_at_timestamp,
            last_update_timestamp: data.updated_at_timestamp,
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
            stop_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: Some(order_type == OrderType::LimitMaker),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[BullishBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.available_quantity.parse().ok();
            let used: Option<Decimal> = b.locked_quantity.parse().ok();
            let total = match (free, used) {
                (Some(f), Some(u)) => Some(f + u),
                (Some(f), None) => Some(f),
                (None, Some(u)) => Some(u),
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

    /// 거래 내역 파싱
    fn parse_trade(&self, trade: &BullishTrade, symbol: &str) -> Trade {
        let timestamp = trade.created_at_timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = trade.price.parse().unwrap_or_default();
        let amount: Decimal = trade.quantity.parse().unwrap_or_default();
        let cost = price * amount;

        let is_taker = trade.is_taker.unwrap_or(true);
        let side_str = trade.side.as_deref().unwrap_or("buy").to_lowercase();

        Trade {
            id: trade.trade_id.clone().unwrap_or_default(),
            order: trade.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side_str),
            taker_or_maker: if is_taker {
                Some(TakerOrMaker::Taker)
            } else {
                Some(TakerOrMaker::Maker)
            },
            price,
            amount,
            cost: Some(cost),
            fee: trade.quote_fee.as_ref().and_then(|f| f.parse().ok()).map(|cost| {
                crate::types::Fee {
                    cost: Some(cost),
                    currency: Some(symbol.split('/').nth(1).unwrap_or("USDC").to_string()),
                    rate: None,
                }
            }),
            fees: Vec::new(),
            info: serde_json::to_value(trade).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Bullish {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bullish
    }

    fn name(&self) -> &str {
        "Bullish"
    }

    fn version(&self) -> &str {
        "v3"
    }

    fn countries(&self) -> &[&str] {
        &["DE"]
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
        let response: Vec<BullishMarket> = self
            .public_get("/v1/markets", None)
            .await?;

        let mut markets = Vec::new();

        for market_data in response {
            // Only include spot markets
            let market_type = market_data.market_type.as_deref().unwrap_or("SPOT");
            if market_type != "SPOT" {
                continue;
            }

            let base = market_data.base_symbol.clone();
            let quote = market_data.quote_symbol.clone();
            let symbol = format!("{}/{}", base, quote);

            let market = Market {
                id: market_data.symbol.clone(),
                lowercase_id: Some(market_data.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: market_data.base_symbol.clone(),
                quote_id: market_data.quote_symbol.clone(),
                settle: None,
                settle_id: None,
                active: market_data.market_enabled.clone().unwrap_or(true),
                market_type: MarketType::Spot,
                spot: true,
                margin: market_data.margin_trading_enabled.clone().unwrap_or(false),
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(1, 3)), // 0.1% default
                maker: Some(Decimal::new(1, 3)), // 0.1% default
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: market_data.quantity_precision.clone().and_then(|p| p.parse().ok()),
                    price: market_data.price_precision.clone().and_then(|p| p.parse().ok()),
                    cost: market_data.cost_precision.clone().and_then(|p| p.parse().ok()),
                    base: market_data.base_precision.clone().and_then(|p| p.parse().ok()),
                    quote: market_data.quote_precision.clone().and_then(|p| p.parse().ok()),
                },
                limits: MarketLimits::default(),
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
        let path = format!("/v1/markets/{}/tick", market_id);

        let response: BullishTicker = self
            .public_get(&path, None)
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/v1/markets/{}/orderbook/hybrid", market_id);

        let response: BullishOrderBook = self
            .public_get(&path, None)
            .await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price.parse().unwrap_or_default(),
                amount: b.price_level_quantity.parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price.parse().unwrap_or_default(),
                amount: a.price_level_quantity.parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: response.timestamp,
            datetime: response.timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: response.sequence_number,
            bids,
            asks,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let path = format!("/v1/history/markets/{}/trades", market_id);

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("_pageSize".into(), l.min(100).to_string());
        }
        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("createdAtDatetime[gte]".into(), datetime);
        }

        let response: Vec<BullishTrade> = self
            .public_get(&path, Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| self.parse_trade(t, symbol))
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
        let path = format!("/v1/markets/{}/candle", market_id);

        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("timeBucket".into(), interval.clone());

        let limit_val = limit.unwrap_or(100).min(100);
        params.insert("_pageSize".into(), limit_val.to_string());

        // Calculate time range
        let until = Utc::now().timestamp_millis();
        let start_time = if let Some(s) = since {
            s
        } else {
            let duration_ms = match timeframe {
                Timeframe::Minute1 => 60_000,
                Timeframe::Minute5 => 300_000,
                Timeframe::Minute30 => 1_800_000,
                Timeframe::Hour1 => 3_600_000,
                Timeframe::Hour6 => 21_600_000,
                Timeframe::Hour12 => 43_200_000,
                Timeframe::Day1 => 86_400_000,
                _ => 3_600_000,
            };
            until - (duration_ms * limit_val as i64)
        };

        let start_datetime = chrono::DateTime::from_timestamp_millis(start_time)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        let until_datetime = chrono::DateTime::from_timestamp_millis(until)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        params.insert("createdAtDatetime[gte]".into(), start_datetime);
        params.insert("createdAtDatetime[lte]".into(), until_datetime);

        let response: Vec<BullishCandle> = self
            .public_get(&path, Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                Some(OHLCV {
                    timestamp: c.created_at_timestamp?,
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

        let response: Vec<BullishBalance> = self
            .private_request("GET", "/v1/accounts/asset", params)
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
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("commandType".into(), "V3CreateOrder".into());
        params.insert("symbol".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }.into());
        params.insert("quantity".into(), amount.to_string());

        let type_str = match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            OrderType::LimitMaker => "POST_ONLY",
            OrderType::StopLossLimit => "STOP_LIMIT",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        };
        params.insert("type".into(), type_str.into());

        if order_type != OrderType::Market {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        params.insert("timeInForce".into(), "GTC".into());

        let response: BullishOrder = self
            .private_request("POST", "/v2/orders", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("commandType".into(), "V3CancelOrder".into());
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BullishOrder = self
            .private_request("POST", "/v2/command", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v2/orders/{}", id);
        let params = HashMap::new();

        let response: BullishOrder = self
            .private_request("GET", &path, params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol = response.symbol.as_ref()
            .and_then(|s| markets_by_id.get(s))
            .cloned()
            .unwrap_or_else(|| response.symbol.clone().unwrap_or_default());

        Ok(self.parse_order(&response, &symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".into(), "OPEN".into());

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("_pageSize".into(), l.min(100).to_string());
        }

        let response: Vec<BullishOrder> = self
            .private_request("GET", "/v2/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = o.symbol.as_ref()
                    .and_then(|s| markets_by_id.get(s))
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone().unwrap_or_default());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();
            params.insert("createdAtDatetime[gte]".into(), datetime);
        }
        if let Some(l) = limit {
            params.insert("_pageSize".into(), l.min(100).to_string());
        }

        let response: Vec<BullishTrade> = self
            .private_request("GET", "/v1/history/trades", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let sym = t.symbol.as_ref()
                    .and_then(|s| markets_by_id.get(s))
                    .map(|s| s.as_str())
                    .unwrap_or(symbol.unwrap_or("UNKNOWN"));
                self.parse_trade(t, sym)
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_time(&self) -> CcxtResult<i64> {
        let response: BullishTime = self
            .public_client
            .get("/v1/time", None, None)
            .await?;

        Ok(response.timestamp)
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
        _method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if let (Some(api_key), Some(secret)) = (self.config.api_key(), self.config.secret()) {
            let timestamp = Utc::now().timestamp_millis().to_string();
            let mut query_params = params.clone();
            query_params.insert("timestamp".into(), timestamp);

            let query: String = query_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            url = format!("{url}?{query}&signature={signature}");
            headers.insert("Authorization".into(), format!("Bearer {}", api_key));
        } else if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        SignedRequest {
            url,
            method: "GET".to_string(),
            headers,
            body: None,
        }
    }
}

// === Bullish API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BullishTime {
    timestamp: i64,
    datetime: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BullishMarket {
    market_id: String,
    symbol: String,
    base_symbol: String,
    quote_symbol: String,
    #[serde(default)]
    base_precision: Option<String>,
    #[serde(default)]
    quote_precision: Option<String>,
    #[serde(default)]
    price_precision: Option<String>,
    #[serde(default)]
    quantity_precision: Option<String>,
    #[serde(default)]
    cost_precision: Option<String>,
    #[serde(default)]
    market_type: Option<String>,
    #[serde(default)]
    market_enabled: Option<bool>,
    #[serde(default)]
    spot_trading_enabled: Option<bool>,
    #[serde(default)]
    margin_trading_enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BullishTicker {
    #[serde(default)]
    created_at_timestamp: Option<i64>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    best_bid: Option<Decimal>,
    #[serde(default)]
    bid_volume: Option<Decimal>,
    #[serde(default)]
    best_ask: Option<Decimal>,
    #[serde(default)]
    ask_volume: Option<Decimal>,
    #[serde(default)]
    vwap: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    change: Option<Decimal>,
    #[serde(default)]
    percentage: Option<Decimal>,
    #[serde(default)]
    average: Option<Decimal>,
    #[serde(default)]
    base_volume: Option<Decimal>,
    #[serde(default)]
    quote_volume: Option<Decimal>,
    #[serde(default)]
    mark_price: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BullishOrderBookLevel {
    price: String,
    price_level_quantity: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BullishOrderBook {
    bids: Vec<BullishOrderBookLevel>,
    asks: Vec<BullishOrderBookLevel>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    sequence_number: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BullishTrade {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    price: String,
    quantity: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    is_taker: Option<bool>,
    #[serde(default)]
    created_at_timestamp: Option<i64>,
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    quote_fee: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BullishCandle {
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
    #[serde(default)]
    created_at_timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BullishOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
    #[serde(default)]
    filled_quantity: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    cumulative_cost: Option<String>,
    #[serde(default)]
    created_at_timestamp: Option<i64>,
    #[serde(default)]
    updated_at_timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BullishBalance {
    asset_id: String,
    asset_symbol: String,
    available_quantity: String,
    locked_quantity: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let bullish = Bullish::new(config).unwrap();

        assert_eq!(bullish.to_market_id("BTC/USDC"), "BTCUSDC");
        assert_eq!(bullish.to_market_id("ETH/USDC"), "ETHUSDC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let bullish = Bullish::new(config).unwrap();

        assert_eq!(bullish.id(), ExchangeId::Bullish);
        assert_eq!(bullish.name(), "Bullish");
        assert!(bullish.has().spot);
        assert!(!bullish.has().margin);
        assert!(!bullish.has().swap);
    }
}
