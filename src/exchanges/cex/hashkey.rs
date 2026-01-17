//! HashKey Global Exchange Implementation
//!
//! CCXT hashkey.ts를 Rust로 포팅

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// HashKey Global 거래소
pub struct Hashkey {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    broker: String,
    recv_window: Option<i64>,
}

impl Hashkey {
    const BASE_URL: &'static str = "https://api-glb.hashkey.com";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 HashKey 인스턴스 생성
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
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/6dd6127b-cc19-4a13-9b29-a98d81f80e98".into()),
            api: api_urls,
            www: Some("https://global.hashkey.com/".into()),
            doc: vec![
                "https://hashkeyglobal-apidoc.readme.io/".into(),
            ],
            fees: Some("https://support.global.hashkey.com/hc/en-us/articles/13199900083612-HashKey-Global-Fee-Structure".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour8, "8h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

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
            broker: "10000700011".into(),
            recv_window: None,
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

        let timestamp = Utc::now().timestamp_millis();

        // Build params with timestamp
        let mut total_params = params.clone();
        total_params.insert("timestamp".into(), timestamp.to_string());

        if let Some(recv_window) = self.recv_window {
            total_params.insert("recvWindow".into(), recv_window.to_string());
        }

        // Create custom URL encoding (replaces %2C with ,)
        let query: String = total_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
            .replace("%2C", ",");

        // Create HMAC-SHA256 signature
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-HK-APIKEY".into(), api_key.to_string());
        headers.insert("INPUT-SOURCE".into(), self.broker.clone());
        headers.insert("broker_sign".into(), signature.clone());

        let url = match method {
            "GET" => {
                let signed_query = format!("{query}&signature={signature}");
                format!("{path}?{signed_query}")
            },
            "POST" | "DELETE" => {
                headers.insert(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                );
                path.to_string()
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("HTTP method: {method}"),
                })
            },
        };

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                let mut form_params = total_params.clone();
                form_params.insert("signature".into(), signature);
                self.private_client
                    .post_form(&url, &form_params, Some(headers))
                    .await
            },
            "DELETE" => {
                let signed_query = format!("{query}&signature={signature}");
                let url_with_query = format!("{path}?{signed_query}");
                self.private_client
                    .delete(&url_with_query, None, Some(headers))
                    .await
            },
            _ => unreachable!(),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTCUSDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &HashkeyTicker, symbol: &str) -> Ticker {
        let timestamp = data.t;

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h,
            low: data.l,
            bid: data.b,
            bid_volume: None,
            ask: data.a,
            ask_volume: None,
            vwap: None,
            open: data.o,
            close: data.c,
            last: data.c,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.v,
            quote_volume: data.qv,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &HashkeyOrder, symbol: &str) -> Order {
        let status = match data.status.as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let mut order_type = match data.order_type.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "LIMIT_MAKER" => OrderType::LimitMaker,
            "STOP" => OrderType::StopLoss,
            _ => OrderType::Limit,
        };

        // If priceType is MARKET, override to market order
        if let Some(price_type) = &data.price_type {
            if price_type == "MARKET" {
                order_type = OrderType::Market;
            }
        }

        let side = match data.side.as_str() {
            "BUY" | "BUY_OPEN" => OrderSide::Buy,
            "SELL" | "SELL_CLOSE" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data
            .time_in_force
            .as_ref()
            .and_then(|tif| match tif.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                _ => None,
            });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.orig_qty.parse().unwrap_or_default();
        let filled: Decimal = data.executed_qty.parse().unwrap_or_default();
        let remaining = Some(amount - filled);

        let average = data.avg_price.as_ref().and_then(|p| {
            let avg: Decimal = p.parse().ok()?;
            if avg > Decimal::ZERO {
                Some(avg)
            } else {
                None
            }
        });

        let cost = average.map(|avg| avg * filled);

        Order {
            id: data.order_id.to_string(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(data.time.unwrap_or(data.transact_time.unwrap_or(0))),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(
                    data.time.unwrap_or(data.transact_time.unwrap_or(0)),
                )
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
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
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
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
    fn parse_balance(&self, balances: &[HashkeyBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.free.parse().ok();
            let used: Option<Decimal> = b.locked.parse().ok();
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
            result.add(&b.asset, balance);
        }

        result
    }

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &HashkeyTrade, symbol: &str) -> Trade {
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.qty.parse().unwrap_or_default();
        let cost = price * amount;

        let side = if data.is_buyer_maker {
            Some("sell".into())
        } else {
            Some("buy".into())
        };

        let taker_or_maker = if data.is_buyer_maker {
            Some(TakerOrMaker::Maker)
        } else {
            Some(TakerOrMaker::Taker)
        };

        Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(data.time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost: Some(cost),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Hashkey {
    fn id(&self) -> ExchangeId {
        ExchangeId::Hashkey
    }

    fn name(&self) -> &str {
        "HashKey Global"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["BM"] // Bermuda
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
        let response: HashkeyExchangeInfo = self.public_get("/api/v1/exchangeInfo", None).await?;

        let mut markets = Vec::new();

        for symbol_info in response.symbols {
            if symbol_info.status != "TRADING" {
                continue;
            }

            let base = symbol_info.base_asset.clone();
            let quote = symbol_info.quote_asset.clone();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: symbol_info.symbol.clone(),
                lowercase_id: Some(symbol_info.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_info.base_asset.clone(),
                quote_id: symbol_info.quote_asset.clone(),
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
                taker: Some(Decimal::new(12, 4)), // 0.0012 = 0.12%
                maker: Some(Decimal::new(12, 4)), // 0.0012 = 0.12%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                precision: MarketPrecision {
                    amount: None,
                    price: None,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_info).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: HashkeyTicker = self
            .public_get("/quote/v1/ticker/24hr", Some(params))
            .await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<HashkeyTicker> = self.public_get("/quote/v1/ticker/24hr", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response {
            if let Some(symbol) = markets_by_id.get(&data.s) {
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

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: HashkeyOrderBook = self.public_get("/quote/v1/depth", Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .b
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .a
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(response.t),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<HashkeyTrade> = self.public_get("/quote/v1/trades", Some(params)).await?;

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
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> =
            self.public_get("/quote/v1/klines", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].as_i64()?,
                    open: c[1].as_str()?.parse().ok()?,
                    high: c[2].as_str()?.parse().ok()?,
                    low: c[3].as_str()?.parse().ok()?,
                    close: c[4].as_str()?.parse().ok()?,
                    volume: c[5].as_str()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: HashkeyAccountInfo = self
            .private_request("GET", "/api/v1/account", params)
            .await?;

        Ok(self.parse_balance(&response.balances))
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
        params.insert("symbol".into(), market_id);
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            }
            .into(),
        );
        params.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "LIMIT",
                OrderType::Market => "MARKET",
                OrderType::LimitMaker => "LIMIT_MAKER",
                OrderType::StopLoss => "STOP",
                _ => {
                    return Err(CcxtError::NotSupported {
                        feature: format!("Order type: {order_type:?}"),
                    })
                },
            }
            .into(),
        );
        params.insert("quantity".into(), amount.to_string());

        if order_type == OrderType::Limit || order_type == OrderType::LimitMaker {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
            params.insert("timeInForce".into(), "GTC".into());
        }

        let response: HashkeyOrder = self
            .private_request("POST", "/api/v1/spot/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: HashkeyOrder = self
            .private_request("DELETE", "/api/v1/spot/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: HashkeyOrder = self
            .private_request("GET", "/api/v1/spot/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: Vec<HashkeyOrder> = self
            .private_request("GET", "/api/v1/spot/openOrders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
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
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "HashKey fetchMyTrades requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<HashkeyMyTrade> = self
            .private_request("GET", "/api/v1/account/trades", params)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.qty.parse().unwrap_or_default();
                let cost = price * amount;

                Trade {
                    id: t.id.to_string(),
                    order: Some(t.order_id.to_string()),
                    timestamp: Some(t.time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: if t.is_buyer {
                        Some("buy".into())
                    } else {
                        Some("sell".into())
                    },
                    taker_or_maker: if t.is_maker {
                        Some(TakerOrMaker::Maker)
                    } else {
                        Some(TakerOrMaker::Taker)
                    },
                    price,
                    amount,
                    cost: Some(cost),
                    fee: t.commission.parse().ok().map(|cost| crate::types::Fee {
                        cost: Some(cost),
                        currency: Some(t.commission_asset.clone()),
                        rate: None,
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
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
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let timestamp = Utc::now().timestamp_millis();
            let mut total_params = params.clone();
            total_params.insert("timestamp".into(), timestamp.to_string());

            if let Some(recv_window) = self.recv_window {
                total_params.insert("recvWindow".into(), recv_window.to_string());
            }

            let query: String = total_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&")
                .replace("%2C", ",");

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            headers.insert("X-HK-APIKEY".into(), api_key.to_string());
            headers.insert("INPUT-SOURCE".into(), self.broker.clone());
            headers.insert("broker_sign".into(), signature.clone());
            headers.insert(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            );

            let signed_query = format!("{query}&signature={signature}");
            if method == "GET" || method == "DELETE" {
                url = format!("{url}?{signed_query}");
            }

            SignedRequest {
                url,
                method: method.to_string(),
                headers,
                body: if method == "POST" {
                    Some(signed_query)
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

// === HashKey API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HashkeyExchangeInfo {
    symbols: Vec<HashkeySymbol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HashkeySymbol {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct HashkeyTicker {
    t: i64,
    s: String,
    c: Option<Decimal>,
    h: Option<Decimal>,
    l: Option<Decimal>,
    o: Option<Decimal>,
    b: Option<Decimal>,
    a: Option<Decimal>,
    v: Option<Decimal>,
    qv: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct HashkeyOrderBook {
    t: i64,
    b: Vec<Vec<String>>,
    a: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HashkeyTrade {
    id: i64,
    price: String,
    qty: String,
    time: i64,
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HashkeyOrder {
    symbol: String,
    order_id: i64,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    orig_qty: String,
    executed_qty: String,
    #[serde(default)]
    avg_price: Option<String>,
    status: String,
    #[serde(rename = "type")]
    order_type: String,
    side: String,
    #[serde(default)]
    time_in_force: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    transact_time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
    #[serde(default)]
    price_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HashkeyAccountInfo {
    balances: Vec<HashkeyBalance>,
}

#[derive(Debug, Deserialize)]
struct HashkeyBalance {
    asset: String,
    free: String,
    locked: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HashkeyMyTrade {
    id: i64,
    order_id: i64,
    price: String,
    qty: String,
    commission: String,
    commission_asset: String,
    time: i64,
    is_buyer: bool,
    is_maker: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let hashkey = Hashkey::new(config).unwrap();

        assert_eq!(hashkey.to_market_id("BTC/USDT"), "BTCUSDT");
        assert_eq!(hashkey.to_market_id("ETH/BTC"), "ETHBTC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let hashkey = Hashkey::new(config).unwrap();

        assert_eq!(hashkey.id(), ExchangeId::Hashkey);
        assert_eq!(hashkey.name(), "HashKey Global");
        assert!(hashkey.has().spot);
        assert!(!hashkey.has().margin);
        assert!(!hashkey.has().swap);
    }
}
