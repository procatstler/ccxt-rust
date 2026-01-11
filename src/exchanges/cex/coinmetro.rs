//! Coinmetro Exchange Implementation
//!
//! CCXT coinmetro.ts를 Rust로 포팅

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
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// Coinmetro 거래소
pub struct Coinmetro {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    currencies_by_id: RwLock<HashMap<String, String>>,
    currency_ids_list: RwLock<Vec<String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Coinmetro {
    const BASE_URL: &'static str = "https://api.coinmetro.com";
    const RATE_LIMIT_MS: u64 = 200; // 1 request per 200 ms

    /// 새 Coinmetro 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: false,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: false,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/ccxt/ccxt/assets/43336371/e86f87ec-6ba3-4410-962b-f7988c5db539".into()),
            api: api_urls,
            www: Some("https://coinmetro.com/".into()),
            doc: vec![
                "https://documenter.getpostman.com/view/3653795/SVfWN6KS".into(),
            ],
            fees: Some("https://help.coinmetro.com/hc/en-gb/articles/6844007317789-What-are-the-fees-on-Coinmetro-".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60000".into());
        timeframes.insert(Timeframe::Minute5, "300000".into());
        timeframes.insert(Timeframe::Minute30, "1800000".into());
        timeframes.insert(Timeframe::Hour4, "14400000".into());
        timeframes.insert(Timeframe::Day1, "86400000".into());

        Ok(Self {
            config,
            public_client,
            private_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            currencies_by_id: RwLock::new(HashMap::new()),
            currency_ids_list: RwLock::new(Vec::new()),
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

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.private_request_with_body(method, path, params, None).await
    }

    /// 비공개 API 호출 (body 포함)
    async fn private_request_with_body<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let token = self.config.token().ok_or_else(|| CcxtError::AuthenticationError {
            message: "JWT token required".into(),
        })?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {token}"));
        headers.insert("Content-Type".into(), "application/json".into());

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

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => self.private_client.post(&url, body, Some(headers)).await,
            "PUT" => self.private_client.put_json(&url, body, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// Market ID 파싱 (BTCEUR -> BTC/EUR)
    fn parse_market_id(&self, market_id: &str) -> (Option<String>, Option<String>) {
        let currency_ids = self.currency_ids_list.read().unwrap();
        let mut sorted_ids = currency_ids.clone();
        // Sort by length (longest first)
        sorted_ids.sort_by_key(|s| std::cmp::Reverse(s.len()));

        for currency_id in &sorted_ids {
            if market_id.starts_with(currency_id) {
                let rest_id = market_id.strip_prefix(currency_id).unwrap();
                if sorted_ids.contains(&rest_id.to_string()) {
                    return (Some(currency_id.clone()), Some(rest_id.to_string()));
                }
            }
        }

        // Fallback for USDT and USD
        if market_id.ends_with("USDT") {
            return (
                Some(market_id.strip_suffix("USDT").unwrap().to_string()),
                Some("USDT".to_string()),
            );
        }
        if market_id.ends_with("USD") {
            return (
                Some(market_id.strip_suffix("USD").unwrap().to_string()),
                Some("USD".to_string()),
            );
        }

        (None, None)
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &CoinmetroTicker, _symbol: &str) -> Ticker {
        let timestamp = data.timestamp;

        Ticker {
            symbol: _symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h,
            low: data.l,
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: None,
            last: data.price,
            previous_close: None,
            change: None,
            percentage: data.delta.map(|d| d * Decimal::from(100)),
            average: None,
            base_volume: data.v,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &CoinmetroOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            Some("open") | Some("placing") | Some("partiallyFilled") => OrderStatus::Open,
            Some("filled") | Some("closed") => OrderStatus::Closed,
            Some("cancelled") | Some("canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            Some("stop") | Some("stopLimit") => OrderType::StopLoss,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
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
            price: data.price,
            average: None,
            amount: data.qty.unwrap_or_default(),
            filled: data.filled_qty.unwrap_or_default(),
            remaining: data.remaining_qty,
            stop_price: data.stop_price,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data.cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[CoinmetroBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let total = b.balance;
            let used = b.allocated.unwrap_or_default();
            let free = total - used;

            let balance = Balance {
                free: Some(free),
                used: Some(used),
                total: Some(total),
                debt: None,
            };
            result.add(&b.currency, balance);
        }

        result
    }

    /// 거래 응답 파싱
    fn parse_trade(&self, data: &CoinmetroTrade, market: Option<&Market>) -> Trade {
        let symbol = market.map(|m| m.symbol.clone()).unwrap_or_default();
        let timestamp = data.timestamp;

        Trade {
            id: data.seq_number.as_ref().map(|n| n.to_string()).unwrap_or_default(),
            order: data.order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol,
            trade_type: None,
            side: data.side.clone(),
            taker_or_maker: None,
            price: data.price,
            amount: data.qty,
            cost: Some(data.price * data.qty),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Coinmetro {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinmetro
    }

    fn name(&self) -> &str {
        "Coinmetro"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["EE"] // Estonia
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
        // Fetch currencies first to build currency mapping
        let currencies_response: Vec<CoinmetroCurrency> = self
            .public_get("/assets", None)
            .await?;

        let mut currencies_by_id = HashMap::new();
        let mut currency_ids = Vec::new();

        for currency in currencies_response {
            let id = currency.symbol.clone();
            currencies_by_id.insert(id.clone(), id.clone());
            currency_ids.push(id);
        }

        {
            let mut by_id = self.currencies_by_id.write().unwrap();
            *by_id = currencies_by_id;
        }
        {
            let mut ids_list = self.currency_ids_list.write().unwrap();
            *ids_list = currency_ids;
        }

        // Now fetch markets
        let response: Vec<CoinmetroMarket> = self
            .public_get("/markets", None)
            .await?;

        let mut markets = Vec::new();

        for market_info in response {
            let id = market_info.pair.clone();
            let (base_id, quote_id) = self.parse_market_id(&id);

            if base_id.is_none() || quote_id.is_none() {
                continue;
            }

            let base_id = base_id.unwrap();
            let quote_id = quote_id.unwrap();
            let symbol = format!("{base_id}/{quote_id}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base_id.clone(),
                quote: quote_id.clone(),
                base_id: base_id.clone(),
                quote_id: quote_id.clone(),
                settle: None,
                settle_id: None,
                active: true,
                market_type: MarketType::Spot,
                spot: true,
                margin: market_info.margin,
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::ZERO),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: None,
                    price: Some(market_info.precision),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, _symbol: &str) -> CcxtResult<Ticker> {
        Err(CcxtError::NotSupported {
            feature: "fetchTicker (use fetchTickers instead)".into(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CoinmetroPricesResponse = self
            .public_get("/exchange/prices", None)
            .await?;

        let markets_by_id = {
            self.markets_by_id.read().unwrap().clone()
        };
        let mut tickers = HashMap::new();

        // Merge latest prices and 24h info
        let mut ticker_map: HashMap<String, CoinmetroTicker> = HashMap::new();

        for price_data in response.latest_prices {
            if let Some(pair) = &price_data.pair {
                ticker_map.insert(pair.clone(), price_data);
            }
        }

        for info_24h in response.info_24h {
            if let Some(pair) = &info_24h.pair {
                if let Some(ticker_data) = ticker_map.get_mut(pair) {
                    ticker_data.delta = info_24h.delta;
                    ticker_data.h = info_24h.h;
                    ticker_data.l = info_24h.l;
                    ticker_data.v = info_24h.v;
                }
            }
        }

        for (market_id, ticker_data) in ticker_map {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&ticker_data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.clone()
        };

        let response: CoinmetroOrderBookResponse = self
            .public_get(&format!("/exchange/book/{}", market.id), None)
            .await?;

        let book = response.book;

        let bids: Vec<OrderBookEntry> = book
            .bid
            .iter()
            .map(|(price_str, amount)| OrderBookEntry {
                price: price_str.parse().unwrap_or_default(),
                amount: *amount,
            })
            .collect();

        let asks: Vec<OrderBookEntry> = book
            .ask
            .iter()
            .map(|(price_str, amount)| OrderBookEntry {
                price: price_str.parse().unwrap_or_default(),
                amount: *amount,
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            nonce: book.seq_number,
            bids,
            asks,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.clone()
        };

        let from = since.map(|s| s.to_string()).unwrap_or_default();

        let response: CoinmetroTradesResponse = self
            .public_get(&format!("/exchange/ticks/{}/{}", market.id, from), None)
            .await?;

        let trades: Vec<Trade> = response
            .tick_history
            .iter()
            .map(|t| self.parse_trade(t, Some(&market)))
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
        let market = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.clone()
        };

        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?.clone();

        let from = since.map(|s| s.to_string()).unwrap_or_else(|| ":from".to_string());

        let to = if let Some(s) = since {
            if let Some(l) = limit {
                let duration = match timeframe {
                    Timeframe::Minute1 => 60000,
                    Timeframe::Minute5 => 300000,
                    Timeframe::Minute30 => 1800000,
                    Timeframe::Hour4 => 14400000,
                    Timeframe::Day1 => 86400000,
                    _ => 60000,
                };
                (s + (duration * l as i64)).to_string()
            } else {
                ":to".to_string()
            }
        } else {
            ":to".to_string()
        };

        let path = format!("/exchange/candles/{}/{}/{}/{}", market.id, interval, from, to);
        let response: CoinmetroOHLCVResponse = self
            .public_get(&path, None)
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .candle_history
            .iter()
            .map(|c| OHLCV {
                timestamp: c.timestamp,
                open: c.o,
                high: c.h,
                low: c.l,
                close: c.c,
                volume: c.v,
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: CoinmetroWalletsResponse = self
            .private_request("GET", "/users/wallets", None)
            .await?;

        Ok(self.parse_balance(&response.list))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.clone()
        };

        let (buying_currency, selling_currency, buying_qty, selling_qty) = match side {
            OrderSide::Buy => {
                (market.base_id.clone(), market.quote_id.clone(), Some(amount), None)
            }
            OrderSide::Sell => {
                (market.quote_id.clone(), market.base_id.clone(), None, Some(amount))
            }
        };

        let mut request_body = serde_json::json!({
            "orderType": match order_type {
                OrderType::Limit => "limit",
                OrderType::Market => "market",
                _ => "limit",
            },
            "buyingCurrency": buying_currency,
            "sellingCurrency": selling_currency,
        });

        if let Some(buy_qty) = buying_qty {
            request_body["buyingQty"] = serde_json::json!(buy_qty.to_string());
        }
        if let Some(sell_qty) = selling_qty {
            request_body["sellingQty"] = serde_json::json!(sell_qty.to_string());
        }

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            request_body["limitPrice"] = serde_json::json!(price_val.to_string());
        }

        let response: CoinmetroOrder = self
            .private_request_with_body("POST", "/exchange/orders/create", None, Some(request_body))
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let response: CoinmetroOrder = self
            .private_request("PUT", &format!("/exchange/orders/cancel/{id}"), None)
            .await?;

        let symbol = {
            let markets_by_id = self.markets_by_id.read().unwrap();
            response.pair.as_ref()
                .and_then(|p| markets_by_id.get(p))
                .cloned()
                .unwrap_or_default()
        };

        Ok(self.parse_order(&response, &symbol))
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let response: CoinmetroOrder = self
            .private_request("GET", &format!("/exchange/orders/status/{id}"), None)
            .await?;

        let symbol = {
            let markets_by_id = self.markets_by_id.read().unwrap();
            response.pair.as_ref()
                .and_then(|p| markets_by_id.get(p))
                .cloned()
                .unwrap_or_default()
        };

        Ok(self.parse_order(&response, &symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let response: Vec<CoinmetroOrder> = self
            .private_request("GET", "/exchange/orders/active", None)
            .await?;

        let markets_by_id = {
            self.markets_by_id.read().unwrap().clone()
        };

        let orders: Vec<Order> = response
            .iter()
            .filter_map(|o| {
                let sym = o.pair.as_ref()
                    .and_then(|p| markets_by_id.get(p))
                    .cloned()
                    .unwrap_or_default();

                if let Some(filter_symbol) = symbol {
                    if sym != filter_symbol {
                        return None;
                    }
                }

                Some(self.parse_order(o, &sym))
            })
            .collect();

        Ok(orders)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        let markets = self.markets.read().unwrap();
        markets.get(symbol).map(|m| m.id.clone())
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
            let token = self.config.token().unwrap_or_default();
            headers.insert("Authorization".into(), format!("Bearer {token}"));
            headers.insert("Content-Type".into(), "application/json".into());
        }

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

    async fn fetch_my_trades(
        &self,
        _symbol: Option<&str>,
        since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let since_param = since.unwrap_or(0).to_string();

        let response: Vec<CoinmetroTrade> = self
            .private_request("GET", &format!("/exchange/fills/{since_param}"), None)
            .await?;

        let markets_by_id = {
            self.markets_by_id.read().unwrap().clone()
        };
        let markets = {
            self.markets.read().unwrap().clone()
        };

        let trades: Vec<Trade> = response
            .iter()
            .filter_map(|t| {
                let market_id = t.pair.as_ref()?;
                let symbol = markets_by_id.get(market_id)?;
                let market = markets.get(symbol)?;
                Some(self.parse_trade(t, Some(market)))
            })
            .collect();

        Ok(trades)
    }
}

// === Coinmetro API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroCurrency {
    symbol: String,
    name: String,
    #[serde(rename = "type")]
    currency_type: String,
    can_deposit: bool,
    can_withdraw: bool,
    can_trade: bool,
    digits: Option<i32>,
    min_qty: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinmetroMarket {
    pair: String,
    precision: i32,
    margin: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroPricesResponse {
    latest_prices: Vec<CoinmetroTicker>,
    #[serde(rename = "24hInfo")]
    info_24h: Vec<Coinmetro24hInfo>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct CoinmetroTicker {
    pair: Option<String>,
    timestamp: i64,
    price: Option<Decimal>,
    qty: Option<Decimal>,
    ask: Option<Decimal>,
    bid: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    h: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    l: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    v: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Coinmetro24hInfo {
    pair: Option<String>,
    delta: Option<Decimal>,
    h: Option<Decimal>,
    l: Option<Decimal>,
    v: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroOrderBookResponse {
    book: CoinmetroOrderBookData,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroOrderBookData {
    pair: String,
    seq_number: Option<i64>,
    ask: HashMap<String, Decimal>,
    bid: HashMap<String, Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroTrade {
    pair: Option<String>,
    price: Decimal,
    qty: Decimal,
    timestamp: i64,
    seq_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroTradesResponse {
    tick_history: Vec<CoinmetroTrade>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroOHLCVResponse {
    candle_history: Vec<CoinmetroCandle>,
}

#[derive(Debug, Deserialize)]
struct CoinmetroCandle {
    pair: String,
    timeframe: i64,
    timestamp: i64,
    c: Decimal,
    h: Decimal,
    l: Decimal,
    o: Decimal,
    v: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmetroOrder {
    #[serde(skip_serializing_if = "Option::is_none")]
    order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qty: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filled_qty: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remaining_qty: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_price: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CoinmetroWalletsResponse {
    list: Vec<CoinmetroBalance>,
}

#[derive(Debug, Deserialize)]
struct CoinmetroBalance {
    currency: String,
    balance: Decimal,
    #[serde(default)]
    allocated: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let coinmetro = Coinmetro::new(config).unwrap();

        assert_eq!(coinmetro.id(), ExchangeId::Coinmetro);
        assert_eq!(coinmetro.name(), "Coinmetro");
        assert!(coinmetro.has().spot);
        assert!(coinmetro.has().margin);
        assert!(!coinmetro.has().swap);
    }
}
