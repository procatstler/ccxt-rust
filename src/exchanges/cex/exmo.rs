//! Exmo Exchange Implementation
//!
//! CCXT exmo.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha512 = Hmac<Sha512>;

/// Exmo 거래소
pub struct Exmo {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    nonce: RwLock<i64>,
}

impl Exmo {
    const BASE_URL: &'static str = "https://api.exmo.com";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per 1 second

    /// 새 Exmo 인스턴스 생성
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
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
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
            logo: Some("https://user-images.githubusercontent.com/1294454/27766491-1b0ea956-5eda-11e7-9225-40d67b481b8d.jpg".into()),
            api: api_urls,
            www: Some("https://exmo.me".into()),
            doc: vec![
                "https://exmo.me/en/api_doc?ref=131685".into(),
            ],
            fees: Some("https://exmo.com/en/docs/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour2, "120".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Day1, "D".into());
        timeframes.insert(Timeframe::Week1, "W".into());
        timeframes.insert(Timeframe::Month1, "M".into());

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
            nonce: RwLock::new(0),
        })
    }

    /// Generate nonce
    fn get_nonce(&self) -> i64 {
        let mut nonce = self.nonce.write().unwrap();
        let now = Utc::now().timestamp_millis();
        if now > *nonce {
            *nonce = now;
        } else {
            *nonce += 1;
        }
        *nonce
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
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
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

        // Add nonce
        let mut body_params = params.clone();
        body_params.insert("nonce".into(), self.get_nonce().to_string());

        // Create POST body
        let body: String = body_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Create HMAC-SHA512 signature
        let mut mac =
            HmacSha512::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(body.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Key".into(), api_key.to_string());
        headers.insert("Sign".into(), signature);

        self.private_client
            .post_form(path, &body_params, Some(headers))
            .await
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTC_USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &ExmoTicker, market_id: &str) -> CcxtResult<Ticker> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol = markets_by_id
            .get(market_id)
            .cloned()
            .unwrap_or_else(|| market_id.replace("_", "/"));

        let timestamp = data
            .updated
            .unwrap_or_else(|| Utc::now().timestamp_millis() / 1000)
            * 1000;

        Ok(Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high,
            low: data.low,
            bid: data.buy_price,
            bid_volume: None,
            ask: data.sell_price,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_trade,
            last: data.last_trade,
            previous_close: None,
            change: None,
            percentage: None,
            average: data.avg,
            base_volume: data.vol,
            quote_volume: data.vol_curr,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        })
    }

    /// 거래 응답 파싱
    fn parse_trade(&self, data: &ExmoTrade, symbol: &str) -> Trade {
        let timestamp = data.date * 1000;
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.quantity.parse().unwrap_or_default();
        let cost: Decimal = data.amount.parse().unwrap_or_default();

        Trade {
            id: data.trade_id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(data.trade_type.clone()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(cost),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &ExmoOrder, symbol: Option<&str>) -> Order {
        let timestamp = data.created * 1000;
        let order_type_str = data.order_type.as_deref().unwrap_or("");

        let side = if order_type_str.contains("buy") {
            OrderSide::Buy
        } else if order_type_str.contains("sell") {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };

        let order_type = if order_type_str.contains("market") {
            OrderType::Market
        } else {
            OrderType::Limit
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data
            .quantity
            .as_ref()
            .and_then(|q| q.parse().ok())
            .unwrap_or_default();
        let cost: Option<Decimal> = data.amount.as_ref().and_then(|a| a.parse().ok());

        let markets_by_id = self.markets_by_id.read().unwrap();
        let symbol_str = if let Some(s) = symbol {
            s.to_string()
        } else if let Some(pair) = &data.pair {
            markets_by_id
                .get(pair)
                .cloned()
                .unwrap_or_else(|| pair.replace("_", "/"))
        } else {
            String::new()
        };

        Order {
            id: data
                .order_id
                .as_ref()
                .unwrap_or(&"".to_string())
                .to_string(),
            client_order_id: data.client_id.map(|id| id.to_string()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol_str,
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
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
    fn parse_balance(
        &self,
        balances: &HashMap<String, String>,
        reserved: &HashMap<String, String>,
    ) -> Balances {
        let mut result = Balances::new();

        for (currency_id, free_str) in balances {
            let free: Option<Decimal> = free_str.parse().ok();
            let used: Option<Decimal> = reserved.get(currency_id).and_then(|s| s.parse().ok());
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
            result.add(currency_id, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Exmo {
    fn id(&self) -> ExchangeId {
        ExchangeId::Exmo
    }

    fn name(&self) -> &str {
        "EXMO"
    }

    fn version(&self) -> &str {
        "v1.1"
    }

    fn countries(&self) -> &[&str] {
        &["LT"] // Lithuania
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
        let response: HashMap<String, ExmoPairSettings> =
            self.public_get("/v1.1/pair_settings", None).await?;

        let mut markets = Vec::new();

        for (id, settings) in response {
            let parts: Vec<&str> = id.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base_id = parts[0];
            let quote_id = parts[1];
            let symbol = format!("{base_id}/{quote_id}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base_id.to_string(),
                quote: quote_id.to_string(),
                base_id: base_id.to_string(),
                quote_id: quote_id.to_string(),
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
                taker: settings
                    .commission_taker_percent
                    .map(|p| p / Decimal::from(100)),
                maker: settings
                    .commission_maker_percent
                    .map(|p| p / Decimal::from(100)),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                precision: MarketPrecision {
                    amount: Some(8),
                    price: settings.price_precision,
                    cost: None,
                    base: Some(8),
                    quote: settings.price_precision,
                },
                limits: MarketLimits {
                    leverage: MinMax {
                        min: None,
                        max: None,
                    },
                    amount: MinMax {
                        min: Some(settings.min_quantity),
                        max: Some(settings.max_quantity),
                    },
                    price: MinMax {
                        min: Some(settings.min_price),
                        max: Some(settings.max_price),
                    },
                    cost: MinMax {
                        min: Some(settings.min_amount),
                        max: Some(settings.max_amount),
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&settings).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let response: HashMap<String, ExmoTicker> = self.public_get("/v1.1/ticker", None).await?;

        let ticker = response
            .get(&market_id)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        self.parse_ticker(ticker, &market_id)
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: HashMap<String, ExmoTicker> = self.public_get("/v1.1/ticker", None).await?;

        let mut tickers = HashMap::new();

        for (market_id, ticker) in response {
            let ticker_parsed = self.parse_ticker(&ticker, &market_id)?;

            if let Some(filter) = symbols {
                if !filter.contains(&ticker_parsed.symbol.as_str()) {
                    continue;
                }
            }

            tickers.insert(ticker_parsed.symbol.clone(), ticker_parsed);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: HashMap<String, ExmoOrderBook> =
            self.public_get("/v1.1/order_book", Some(params)).await?;

        let orderbook = response
            .get(&market_id)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let bids: Vec<OrderBookEntry> = orderbook
            .bid
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = orderbook
            .ask
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
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
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id.clone());

        let response: HashMap<String, Vec<ExmoTrade>> =
            self.public_get("/v1.1/trades", Some(params)).await?;

        let trades_data = response
            .get(&market_id)
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;

        let trades: Vec<Trade> = trades_data
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

        let duration = match timeframe {
            Timeframe::Minute1 => 60,
            Timeframe::Minute5 => 300,
            Timeframe::Minute15 => 900,
            Timeframe::Minute30 => 1800,
            Timeframe::Hour1 => 3600,
            Timeframe::Hour2 => 7200,
            Timeframe::Hour4 => 14400,
            Timeframe::Day1 => 86400,
            Timeframe::Week1 => 604800,
            Timeframe::Month1 => 2592000,
            _ => 60,
        };

        let now = Utc::now().timestamp();
        let from = if let Some(s) = since {
            s / 1000
        } else {
            let lim = limit.unwrap_or(1000) as i64;
            now - (lim * duration)
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("resolution".into(), interval.clone());
        params.insert("from".into(), from.to_string());
        params.insert("to".into(), now.to_string());

        let response: ExmoCandles = self
            .public_get("/v1.1/candles_history", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .candles
            .iter()
            .map(|c| OHLCV {
                timestamp: c.t,
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
        let params = HashMap::new();

        let response: ExmoUserInfo = self.private_post("/v1.1/user_info", params).await?;

        Ok(self.parse_balance(&response.balances, &response.reserved))
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
        params.insert("pair".into(), market_id.clone());
        params.insert("quantity".into(), amount.to_string());

        let type_str = match (order_type, side) {
            (OrderType::Market, OrderSide::Buy) => "market_buy",
            (OrderType::Market, OrderSide::Sell) => "market_sell",
            (OrderType::Limit, OrderSide::Buy) => "buy",
            (OrderType::Limit, OrderSide::Sell) => "sell",
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            },
        };
        params.insert("type".into(), type_str.into());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        } else {
            params.insert("price".into(), "0".into());
        }

        let response: ExmoOrderResponse = self.private_post("/v1.1/order_create", params).await?;

        let order = ExmoOrder {
            order_id: Some(response.order_id.to_string()),
            created: Utc::now().timestamp(),
            order_type: Some(type_str.to_string()),
            pair: Some(market_id),
            price: price.map(|p| p.to_string()),
            quantity: Some(amount.to_string()),
            amount: None,
            client_id: None,
            stop_price: None,
            trigger_price: None,
        };

        Ok(self.parse_order(&order, Some(symbol)))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: serde_json::Value = self.private_post("/v1.1/order_cancel", params).await?;

        let order = ExmoOrder {
            order_id: Some(id.to_string()),
            created: Utc::now().timestamp(),
            order_type: None,
            pair: None,
            price: None,
            quantity: None,
            amount: None,
            client_id: None,
            stop_price: None,
            trigger_price: None,
        };

        let mut parsed_order = self.parse_order(&order, None);
        parsed_order.status = OrderStatus::Canceled;
        parsed_order.info = response;

        Ok(parsed_order)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: ExmoOrderTradesResponse =
            self.private_post("/v1.1/order_trades", params).await?;

        let order = ExmoOrder {
            order_id: Some(id.to_string()),
            created: Utc::now().timestamp(),
            order_type: Some(response.order_type),
            pair: response.trades.first().map(|t| t.pair.clone()),
            price: None,
            quantity: None,
            amount: None,
            client_id: None,
            stop_price: None,
            trigger_price: None,
        };

        Ok(self.parse_order(&order, None))
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let params = HashMap::new();

        let response: HashMap<String, Vec<ExmoOrder>> =
            self.private_post("/v1.1/user_open_orders", params).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut orders = Vec::new();

        for (market_id, market_orders) in response {
            let symbol = markets_by_id.get(&market_id).map(|s| s.as_str());

            for order_data in market_orders {
                orders.push(self.parse_order(&order_data, symbol));
            }
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Exmo fetchMyTrades requires a symbol argument".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: HashMap<String, Vec<ExmoMyTrade>> =
            self.private_post("/v1.1/user_trades", params).await?;

        let trades_data = response
            .get(&market_id)
            .ok_or_else(|| CcxtError::ExchangeError {
                message: format!("No trades found for {symbol}"),
            })?;

        let trades: Vec<Trade> = trades_data
            .iter()
            .map(|t| {
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.quantity.parse().unwrap_or_default();
                let cost = price * amount;

                Trade {
                    id: t.trade_id.to_string(),
                    order: Some(t.order_id.to_string()),
                    timestamp: Some(t.date * 1000),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.date * 1000)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.trade_type.clone()),
                    taker_or_maker: Some(if t.exec_type == "maker" {
                        TakerOrMaker::Maker
                    } else {
                        TakerOrMaker::Taker
                    }),
                    price,
                    amount,
                    cost: Some(cost),
                    fee: Some(crate::types::Fee {
                        cost: t.commission_amount.parse().ok(),
                        currency: Some(t.commission_currency.clone()),
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
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("{}{}", Self::BASE_URL, path);
        let headers = HashMap::new();

        if method == "GET" {
            SignedRequest {
                url,
                method: method.to_string(),
                headers,
                body: None,
            }
        } else {
            // POST requires signature - handled in private_post
            SignedRequest {
                url,
                method: method.to_string(),
                headers,
                body: None,
            }
        }
    }
}

// === Exmo API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct ExmoPairSettings {
    min_quantity: Decimal,
    max_quantity: Decimal,
    min_price: Decimal,
    max_price: Decimal,
    min_amount: Decimal,
    max_amount: Decimal,
    price_precision: Option<i32>,
    commission_taker_percent: Option<Decimal>,
    commission_maker_percent: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExmoTicker {
    buy_price: Option<Decimal>,
    sell_price: Option<Decimal>,
    last_trade: Option<Decimal>,
    high: Option<Decimal>,
    low: Option<Decimal>,
    avg: Option<Decimal>,
    vol: Option<Decimal>,
    vol_curr: Option<Decimal>,
    updated: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ExmoOrderBook {
    ask_quantity: String,
    ask_amount: String,
    ask_top: String,
    bid_quantity: String,
    bid_amount: String,
    bid_top: String,
    ask: Vec<Vec<String>>,
    bid: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExmoTrade {
    trade_id: i64,
    #[serde(rename = "type")]
    trade_type: String,
    price: String,
    quantity: String,
    amount: String,
    date: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExmoOrder {
    order_id: Option<String>,
    created: i64,
    #[serde(rename = "type")]
    order_type: Option<String>,
    pair: Option<String>,
    price: Option<String>,
    quantity: Option<String>,
    amount: Option<String>,
    client_id: Option<i64>,
    stop_price: Option<String>,
    trigger_price: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExmoUserInfo {
    balances: HashMap<String, String>,
    reserved: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ExmoOrderResponse {
    result: bool,
    error: String,
    order_id: i64,
}

#[derive(Debug, Deserialize)]
struct ExmoOrderTradesResponse {
    #[serde(rename = "type")]
    order_type: String,
    in_currency: String,
    in_amount: String,
    out_currency: String,
    out_amount: String,
    trades: Vec<ExmoTradeDetail>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExmoTradeDetail {
    trade_id: i64,
    date: i64,
    #[serde(rename = "type")]
    trade_type: String,
    pair: String,
    order_id: i64,
    quantity: String,
    price: String,
    amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExmoMyTrade {
    trade_id: i64,
    date: i64,
    #[serde(rename = "type")]
    trade_type: String,
    pair: String,
    order_id: i64,
    quantity: String,
    price: String,
    amount: String,
    exec_type: String,
    commission_amount: String,
    commission_currency: String,
    commission_percent: String,
}

#[derive(Debug, Deserialize)]
struct ExmoCandles {
    candles: Vec<ExmoCandle>,
}

#[derive(Debug, Deserialize)]
struct ExmoCandle {
    t: i64,
    o: Decimal,
    c: Decimal,
    h: Decimal,
    l: Decimal,
    v: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exmo = Exmo::new(config).unwrap();

        assert_eq!(exmo.to_market_id("BTC/USDT"), "BTC_USDT");
        assert_eq!(exmo.to_market_id("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exmo = Exmo::new(config).unwrap();

        assert_eq!(exmo.id(), ExchangeId::Exmo);
        assert_eq!(exmo.name(), "EXMO");
        assert!(exmo.has().spot);
        assert!(exmo.has().margin);
        assert!(!exmo.has().swap);
    }
}
