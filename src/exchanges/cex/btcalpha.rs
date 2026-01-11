//! BTC-Alpha Exchange Implementation
//!
//! CCXT btcalpha.ts를 Rust로 포팅

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
    MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, Transaction, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// BTC-Alpha 거래소
pub struct Btcalpha {
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

impl Btcalpha {
    const BASE_URL: &'static str = "https://btc-alpha.com/api";
    const RATE_LIMIT_MS: u64 = 100; // Conservative rate limit

    /// 새 Btcalpha 인스턴스 생성
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
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/dce49f3a-61e5-4ba0-a2fe-41d192fd0e5d"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://btc-alpha.com".into()),
            doc: vec!["https://btc-alpha.github.io/api-docs".into()],
            fees: Some("https://btc-alpha.com/fees/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Day1, "D".into());

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

        let nonce = Utc::now().timestamp_millis().to_string();

        // Build query string for GET or body for POST
        let query: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Create payload for signature
        let payload = if method == "POST" {
            format!("{api_key}{query}")
        } else {
            api_key.to_string()
        };

        // Create HMAC-SHA256 signature
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-KEY".into(), api_key.to_string());
        headers.insert("X-SIGN".into(), signature);
        headers.insert("X-NONCE".into(), nonce);
        headers.insert("Accept".into(), "application/json".into());

        match method {
            "GET" => {
                let url = if query.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}?{query}")
                };
                self.private_client.get(&url, None, Some(headers)).await
            },
            "POST" => {
                self.private_client
                    .post_form(path, &params, Some(headers))
                    .await
            },
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
    fn to_symbol(&self, _market_id: &str, base: &str, quote: &str) -> String {
        format!("{base}/{quote}")
    }

    /// Parse precision from decimal places
    fn parse_precision(&self, precision: i32) -> Decimal {
        Decimal::new(1, precision as u32)
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BtcalphaTicker, symbol: &str) -> Ticker {
        // Parse timestamp from float seconds to milliseconds
        let timestamp_str = data.timestamp.clone();
        let timestamp = if let Ok(ts_float) = timestamp_str.parse::<f64>() {
            (ts_float * 1_000_000.0) as i64
        } else {
            Utc::now().timestamp_millis()
        };

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
            bid: data.buy,
            bid_volume: None,
            ask: data.sell,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.diff,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BtcalphaOrder, symbol: Option<&str>) -> Order {
        let status = match data.status.as_deref() {
            Some("1") => OrderStatus::Open,
            Some("2") => OrderStatus::Canceled,
            Some("3") => OrderStatus::Closed,
            _ => OrderStatus::Open,
        };

        let side = match data.my_side.as_deref().or(data.order_type.as_deref()) {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        // Determine symbol from pair or use provided
        let order_symbol = if let Some(pair) = &data.pair {
            let parts: Vec<&str> = pair.split('_').collect();
            if parts.len() == 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                symbol.unwrap_or("").to_string()
            }
        } else {
            symbol.unwrap_or("").to_string()
        };

        let timestamp = if data.success.unwrap_or(false) {
            data.date
                .as_ref()
                .and_then(|d| d.parse::<f64>().ok().map(|ts| (ts * 1000.0) as i64))
        } else {
            data.date.as_ref().and_then(|d| d.parse::<i64>().ok())
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data
            .amount_original
            .as_ref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_else(|| {
                data.amount
                    .as_ref()
                    .and_then(|a| a.parse().ok())
                    .unwrap_or_default()
            });
        let filled: Decimal = data
            .amount_filled
            .as_ref()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();
        let remaining: Decimal = data
            .amount
            .as_ref()
            .and_then(|r| r.parse().ok())
            .unwrap_or_else(|| amount - filled);

        Order {
            id: data
                .oid
                .as_ref()
                .or(data.id.as_ref())
                .or(data.order.as_ref())
                .cloned()
                .unwrap_or_default(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: order_symbol,
            order_type: OrderType::Limit, // BTC-Alpha only supports limit orders
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining: Some(remaining),
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
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &BtcalphaTrade, symbol: Option<&str>) -> Trade {
        let timestamp_str = data.timestamp.clone();
        let timestamp = if let Ok(ts_float) = timestamp_str.parse::<f64>() {
            (ts_float * 1_000_000.0) as i64
        } else {
            Utc::now().timestamp_millis()
        };

        let trade_symbol = if let Some(pair) = &data.pair {
            let parts: Vec<&str> = pair.split('_').collect();
            if parts.len() == 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                symbol.unwrap_or("").to_string()
            }
        } else {
            symbol.unwrap_or("").to_string()
        };

        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();
        let cost = price * amount;

        let side = data
            .my_side
            .as_deref()
            .or(data.trade_type.as_deref())
            .map(String::from);

        Trade {
            id: data.id.clone(),
            order: Some(data.id.clone()),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: trade_symbol,
            trade_type: None,
            side,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(cost),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[BtcalphaBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let total: Option<Decimal> = b.balance.parse().ok();
            let used: Option<Decimal> = b.reserve.parse().ok();
            let free = match (total, used) {
                (Some(t), Some(u)) => Some(t - u),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.currency, balance);
        }

        result
    }

    /// Parse transaction (deposit/withdrawal)
    fn parse_transaction(
        &self,
        data: &BtcalphaTransaction,
        tx_type: crate::types::TransactionType,
    ) -> Transaction {
        let status = match data.status {
            Some(10) => crate::types::TransactionStatus::Pending, // New
            Some(20) => crate::types::TransactionStatus::Pending, // Verified
            Some(30) => crate::types::TransactionStatus::Ok,      // Approved
            Some(40) => crate::types::TransactionStatus::Failed,  // Refused
            Some(50) => crate::types::TransactionStatus::Canceled, // Cancelled
            _ => crate::types::TransactionStatus::Pending,
        };

        let timestamp = if let Ok(ts_float) = data.timestamp.parse::<f64>() {
            (ts_float * 1000.0) as i64
        } else {
            Utc::now().timestamp_millis()
        };

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            updated: None,
            tx_type,
            currency: data.currency.clone(),
            network: None,
            amount: data.amount.parse().unwrap_or_default(),
            status,
            address: None,
            tag: None,
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Btcalpha {
    fn id(&self) -> ExchangeId {
        ExchangeId::Btcalpha
    }

    fn name(&self) -> &str {
        "BTC-Alpha"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["US"]
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
        let response: Vec<BtcalphaMarket> = self.public_get("/v1/pairs/", None).await?;

        let mut markets = Vec::new();

        for market_info in response {
            let base = market_info.currency1.clone();
            let quote = market_info.currency2.clone();
            let symbol = format!("{base}/{quote}");

            let price_precision = market_info.price_precision;
            let amount_precision = market_info.amount_precision;

            let price_limit = self.parse_precision(price_precision);
            let amount_limit: Decimal = market_info.minimum_order_size.parse().unwrap_or_default();

            let market = Market {
                id: market_info.name.clone(),
                lowercase_id: Some(market_info.name.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: market_info.currency1.clone(),
                quote_id: market_info.currency2.clone(),
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
                taker: Some(Decimal::new(2, 3)), // 0.2%
                maker: Some(Decimal::new(2, 3)), // 0.2%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: None,
                    base: Some(amount_precision),
                    quote: Some(price_precision),
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: Some(amount_limit),
                        max: market_info.maximum_order_size.parse().ok(),
                    },
                    price: MinMax {
                        min: Some(price_limit),
                        max: None,
                    },
                    cost: MinMax {
                        min: market_info.minimum_order_value.parse().ok(),
                        max: None,
                    },
                    ..Default::default()
                },
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

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        let response: BtcalphaTicker = self.public_get("/v1/ticker/", Some(params)).await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<BtcalphaTicker> = self.public_get("/v1/ticker/", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for data in response {
            if let Some(symbol) = markets_by_id.get(&data.pair) {
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

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();

        if let Some(l) = limit {
            params.insert("limit_sell".into(), l.to_string());
            params.insert("limit_buy".into(), l.to_string());
        }

        let path = format!("/v1/orderbook/{market_id}/");
        let response: BtcalphaOrderBook = self
            .public_get(
                &path,
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        let bids: Vec<OrderBookEntry> = response
            .buy
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price.parse().unwrap_or_default(),
                amount: b.amount.parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .sell
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price.parse().unwrap_or_default(),
                amount: a.amount.parse().unwrap_or_default(),
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
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BtcalphaTrade> = self.public_get("/v1/exchanges/", Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| self.parse_trade(t, Some(symbol)))
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
        if let Some(s) = since {
            params.insert("since".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(720).to_string());
        }

        let path = format!("/v1/charts/{market_id}/{interval}/chart/");
        let response: Vec<BtcalphaOHLCV> = self
            .public_get(
                &path,
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .map(|c| OHLCV {
                timestamp: c.time * 1000,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume,
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: Vec<BtcalphaBalance> =
            self.private_request("GET", "/v1/wallets/", params).await?;

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
        if order_type != OrderType::Limit {
            return Err(CcxtError::InvalidOrder {
                message: "BTC-Alpha only supports limit orders".into(),
            });
        }

        let market_id = self.to_market_id(symbol);
        let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Limit order requires price".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert(
            "type".into(),
            match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }
            .into(),
        );
        params.insert("amount".into(), amount.to_string());
        params.insert("price".into(), price_val.to_string());

        let response: BtcalphaOrder = self.private_request("POST", "/v1/order/", params).await?;

        if !response.success.unwrap_or(false) {
            return Err(CcxtError::InvalidOrder {
                message: "Order creation failed".into(),
            });
        }

        let mut order = self.parse_order(&response, Some(symbol));
        // Set amount from request if response amount is 0
        if order.amount == Decimal::ZERO {
            order.amount = amount;
        }

        Ok(order)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order".into(), id.to_string());

        let response: BtcalphaOrder = self
            .private_request("POST", "/v1/order-cancel/", params)
            .await?;

        Ok(self.parse_order(&response, None))
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/order/{id}/");
        let params = HashMap::new();

        let response: BtcalphaOrder = self.private_request("GET", &path, params).await?;

        Ok(self.parse_order(&response, None))
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            params.insert("pair".into(), market_id);
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BtcalphaOrder> = self
            .private_request("GET", "/v1/orders/own/", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = o
                    .pair
                    .as_ref()
                    .and_then(|p| markets_by_id.get(p))
                    .map(|s| s.as_str());
                self.parse_order(o, sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".into(), "1".into());

        if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            params.insert("pair".into(), market_id);
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BtcalphaOrder> = self
            .private_request("GET", "/v1/orders/own/", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = o
                    .pair
                    .as_ref()
                    .and_then(|p| markets_by_id.get(p))
                    .map(|s| s.as_str());
                self.parse_order(o, sym)
            })
            .filter(|o| {
                if let Some(s) = since {
                    if let Some(ts) = o.timestamp {
                        ts >= s
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".into(), "3".into());

        if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            params.insert("pair".into(), market_id);
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BtcalphaOrder> = self
            .private_request("GET", "/v1/orders/own/", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let sym = o
                    .pair
                    .as_ref()
                    .and_then(|p| markets_by_id.get(p))
                    .map(|s| s.as_str());
                self.parse_order(o, sym)
            })
            .filter(|o| {
                if let Some(s) = since {
                    if let Some(ts) = o.timestamp {
                        ts >= s
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
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

        if let Some(s) = symbol {
            let market_id = self.to_market_id(s);
            params.insert("pair".into(), market_id);
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BtcalphaTrade> = self
            .private_request("GET", "/v1/exchanges/own/", params)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        _code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let params = HashMap::new();

        let response: Vec<BtcalphaTransaction> =
            self.private_request("GET", "/v1/deposits/", params).await?;

        let transactions: Vec<Transaction> = response
            .iter()
            .map(|d| self.parse_transaction(d, crate::types::TransactionType::Deposit))
            .collect();

        Ok(transactions)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("currency_id".into(), c.to_string());
        }

        let response: Vec<BtcalphaTransaction> = self
            .private_request("GET", "/v1/withdraws/", params)
            .await?;

        let transactions: Vec<Transaction> = response
            .iter()
            .map(|w| self.parse_transaction(w, crate::types::TransactionType::Withdrawal))
            .collect();

        Ok(transactions)
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
        headers.insert("Accept".into(), "application/json".into());

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();
            let nonce = Utc::now().timestamp_millis().to_string();

            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let payload = if method == "POST" {
                format!("{api_key}{query}")
            } else {
                api_key.to_string()
            };

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(payload.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            headers.insert("X-KEY".into(), api_key.to_string());
            headers.insert("X-SIGN".into(), signature);
            headers.insert("X-NONCE".into(), nonce);

            if method == "POST" {
                headers.insert(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                );
            } else if !query.is_empty() {
                url = format!("{url}?{query}");
            }
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
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// === BTC-Alpha API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaMarket {
    name: String,
    currency1: String,
    currency2: String,
    price_precision: i32,
    amount_precision: i32,
    minimum_order_size: String,
    maximum_order_size: String,
    minimum_order_value: String,
    #[serde(default)]
    liquidity_type: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaTicker {
    timestamp: String,
    pair: String,
    last: Option<Decimal>,
    diff: Option<Decimal>,
    vol: Option<Decimal>,
    high: Option<Decimal>,
    low: Option<Decimal>,
    buy: Option<Decimal>,
    sell: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BtcalphaOrderBook {
    buy: Vec<BtcalphaBookEntry>,
    sell: Vec<BtcalphaBookEntry>,
}

#[derive(Debug, Deserialize)]
struct BtcalphaBookEntry {
    price: String,
    amount: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaTrade {
    id: String,
    timestamp: String,
    pair: Option<String>,
    price: String,
    amount: String,
    #[serde(rename = "type")]
    trade_type: Option<String>,
    #[serde(default)]
    my_side: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(rename = "type")]
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    amount_filled: Option<String>,
    #[serde(default)]
    amount_original: Option<String>,
    #[serde(default)]
    my_side: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    trades: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct BtcalphaBalance {
    currency: String,
    balance: String,
    reserve: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcalphaTransaction {
    #[serde(default)]
    id: Option<String>,
    timestamp: String,
    currency: String,
    amount: String,
    #[serde(default)]
    status: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct BtcalphaOHLCV {
    time: i64,
    open: Decimal,
    close: Decimal,
    low: Decimal,
    high: Decimal,
    volume: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let btcalpha = Btcalpha::new(config).unwrap();

        assert_eq!(btcalpha.to_market_id("BTC/USDT"), "BTC_USDT");
        assert_eq!(btcalpha.to_market_id("ETH/BTC"), "ETH_BTC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let btcalpha = Btcalpha::new(config).unwrap();

        assert_eq!(btcalpha.id(), ExchangeId::Btcalpha);
        assert_eq!(btcalpha.name(), "BTC-Alpha");
        assert!(btcalpha.has().spot);
        assert!(!btcalpha.has().margin);
        assert!(!btcalpha.has().swap);
    }
}
