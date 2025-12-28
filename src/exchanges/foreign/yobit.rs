//! Yobit Exchange Implementation
//!
//! CCXT yobit.ts를 Rust로 포팅

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha512 = Hmac<Sha512>;

/// Yobit 거래소
pub struct Yobit {
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

impl Yobit {
    const BASE_URL: &'static str = "https://yobit.net";
    const RATE_LIMIT_MS: u64 = 2000; // 2 seconds - responses are cached

    /// 새 Yobit 인스턴스 생성
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
            fetch_currencies: false,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false, // Yobit does not support market orders
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: false,
            fetch_withdrawals: false,
            withdraw: true,
            fetch_deposit_address: true,
            ws: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), "https://yobit.net/api".into());
        api_urls.insert("private".into(), "https://yobit.net/tapi".into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766910-cdcbfdae-5eea-11e7-9859-03fea873272d.jpg".into()),
            api: api_urls,
            www: Some("https://www.yobit.net".into()),
            doc: vec!["https://www.yobit.net/en/api/".into()],
            fees: Some("https://www.yobit.net/en/fees/".into()),
        };

        let timeframes = HashMap::new(); // Yobit does not support OHLCV

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
            nonce: RwLock::new(Utc::now().timestamp_millis()),
        })
    }

    /// Get next nonce
    fn next_nonce(&self) -> i64 {
        let mut nonce = self.nonce.write().unwrap();
        *nonce += 1;
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
        method: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = self.next_nonce();
        let mut body_params = params.clone();
        body_params.insert("nonce".into(), nonce.to_string());
        body_params.insert("method".into(), method.to_string());

        let body: String = body_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Create HMAC-SHA512 signature
        let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(body.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Key".into(), api_key.to_string());
        headers.insert("Sign".into(), signature);

        let response: YobitResponse<T> = self
            .private_client
            .post_form("", &body_params, Some(headers))
            .await?;

        if response.success == 1 {
            response.return_value.ok_or_else(|| CcxtError::ExchangeError {
                message: "Missing return value in response".into(),
            })
        } else {
            Err(CcxtError::ExchangeError {
                message: response.error.unwrap_or_else(|| "Unknown error".into()),
            })
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → btc_usdt)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.to_lowercase().replace("/", "_")
    }

    /// 마켓 ID → 심볼 변환 (btc_usdt → BTC/USDT)
    fn to_symbol(&self, _market_id: &str, base: &str, quote: &str) -> String {
        format!("{}/{}", base.to_uppercase(), quote.to_uppercase())
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &YobitTickerData, symbol: &str) -> Ticker {
        let timestamp = data.updated.map(|t| t * 1000); // Convert to milliseconds

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: timestamp.and_then(|t|
                chrono::DateTime::<Utc>::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
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
            change: None,
            percentage: None,
            average: data.avg,
            base_volume: data.vol_cur,
            quote_volume: data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &YobitOrderInfo, symbol: &str, order_id: Option<&str>) -> Order {
        let status = match data.status.unwrap_or(0) {
            0 => OrderStatus::Open,
            1 => OrderStatus::Closed,
            2 => OrderStatus::Canceled,
            3 => OrderStatus::Open, // partially filled
            _ => OrderStatus::Open,
        };

        let side = match data.order_type.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data.rate.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.start_amount.as_ref()
            .or(data.amount.as_ref())
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();
        let remaining: Decimal = data.amount.as_ref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();
        let filled = amount - remaining;

        Order {
            id: order_id.unwrap_or_default().to_string(),
            client_order_id: None,
            timestamp: data.timestamp_created.map(|t| t * 1000),
            datetime: data.timestamp_created.and_then(|t|
                chrono::DateTime::from_timestamp_millis(t * 1000)
                    .map(|dt| dt.to_rfc3339())
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit, // Yobit only supports limit orders
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
            cost: price.map(|p| p * filled),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 거래 응답 파싱
    fn parse_trade(&self, data: &YobitTrade, symbol: &str, trade_id: Option<&str>) -> Trade {
        let timestamp = data.timestamp.map(|t| t * 1000);

        let side = match data.trade_type.as_deref() {
            Some("ask") => Some("sell".into()),
            Some("bid") => Some("buy".into()),
            Some("sell") => Some("sell".into()),
            Some("buy") => Some("buy".into()),
            _ => None,
        };

        let price: Decimal = data.price.as_ref()
            .or(data.rate.as_ref())
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data.amount.as_ref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();

        Trade {
            id: trade_id.unwrap_or_default().to_string(),
            order: data.order_id.as_ref().map(|id| id.to_string()),
            timestamp,
            datetime: timestamp.and_then(|t|
                chrono::DateTime::<Utc>::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            ),
            symbol: symbol.to_string(),
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
    fn parse_balance(&self, data: &YobitBalanceInfo) -> Balances {
        let mut result = Balances::new();

        // Parse free balances
        if let Some(free_funds) = &data.funds {
            for (currency, amount_str) in free_funds {
                let free: Option<Decimal> = amount_str.parse().ok();
                let currency_upper = currency.to_uppercase();

                let balance = Balance {
                    free,
                    used: None,
                    total: None,
                    debt: None,
                };
                result.add(&currency_upper, balance);
            }
        }

        // Update with total balances including orders
        if let Some(total_funds) = &data.funds_incl_orders {
            for (currency, amount_str) in total_funds {
                let total: Option<Decimal> = amount_str.parse().ok();
                let currency_upper = currency.to_uppercase();

                if let Some(existing) = result.get(&currency_upper) {
                    let free = existing.free;
                    let used = match (total, free) {
                        (Some(t), Some(f)) => Some(t - f),
                        _ => None,
                    };

                    let balance = Balance {
                        free,
                        used,
                        total,
                        debt: None,
                    };
                    result.add(&currency_upper, balance);
                } else {
                    let balance = Balance {
                        free: None,
                        used: None,
                        total,
                        debt: None,
                    };
                    result.add(&currency_upper, balance);
                }
            }
        }

        result
    }
}

#[async_trait]
impl Exchange for Yobit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Yobit
    }

    fn name(&self) -> &str {
        "YoBit"
    }

    fn version(&self) -> &str {
        "3"
    }

    fn countries(&self) -> &[&str] {
        &["RU"]
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
        let response: YobitInfoResponse = self
            .public_get("/api/3/info", None)
            .await?;

        let mut markets = Vec::new();

        for (market_id, market_info) in response.pairs {
            let parts: Vec<&str> = market_id.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base_id = parts[0];
            let quote_id = parts[1];
            let base = base_id.to_uppercase();
            let quote = quote_id.to_uppercase();
            let symbol = format!("{}/{}", base, quote);

            let _fee_string = market_info.fee.to_string();
            let fee = Decimal::new(market_info.fee.round() as i64, 3); // fee / 100

            let active = market_info.hidden == 0;

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.clone()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base_id.to_string(),
                quote_id: quote_id.to_string(),
                settle: None,
                settle_id: None,
                active,
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
                taker: Some(fee),
                maker: Some(fee),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(market_info.decimal_places),
                    price: Some(market_info.decimal_places),
                    cost: None,
                    base: Some(market_info.decimal_places),
                    quote: Some(market_info.decimal_places),
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: Some(market_info.min_amount),
                        max: market_info.max_amount,
                    },
                    price: MinMax {
                        min: Some(market_info.min_price),
                        max: Some(market_info.max_price),
                    },
                    cost: MinMax {
                        min: Some(market_info.min_total),
                        max: None,
                    },
                    leverage: MinMax {
                        min: None,
                        max: None,
                    },
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
        let path = format!("/api/3/ticker/{}", market_id);

        let response: HashMap<String, YobitTickerData> = self
            .public_get(&path, None)
            .await?;

        let ticker_data = response.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Ticker data not found for {}", symbol),
        })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        // Yobit requires symbols to be specified
        let market_ids: String = if let Some(syms) = symbols {
            syms.iter()
                .map(|s| self.to_market_id(s))
                .collect::<Vec<_>>()
                .join("-")
        } else {
            return Err(CcxtError::ArgumentsRequired {
                message: "Yobit fetchTickers requires symbols argument".into(),
            });
        };

        let path = format!("/api/3/ticker/{}", market_ids);
        let response: HashMap<String, YobitTickerData> = self
            .public_get(&path, None)
            .await?;

        // Clone markets_by_id after the await
        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let mut tickers = HashMap::new();
        for (market_id, ticker_data) in response {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                let ticker = self.parse_ticker(&ticker_data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut path = format!("/api/3/depth/{}", market_id);

        if let Some(l) = limit {
            path = format!("{}?limit={}", path, l);
        }

        let response: HashMap<String, YobitOrderBookData> = self
            .public_get(&path, None)
            .await?;

        let orderbook_data = response.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Order book not found for {}", symbol),
        })?;

        let bids: Vec<OrderBookEntry> = orderbook_data
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0],
                amount: b[1],
            })
            .collect();

        let asks: Vec<OrderBookEntry> = orderbook_data
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0],
                amount: a[1],
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
        let mut path = format!("/api/3/trades/{}", market_id);

        if let Some(l) = limit {
            path = format!("{}?limit={}", path, l);
        }

        let response: HashMap<String, Vec<YobitPublicTrade>> = self
            .public_get(&path, None)
            .await?;

        let trades_data = response.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Trades not found for {}", symbol),
        })?;

        let trades: Vec<Trade> = trades_data
            .iter()
            .map(|t| {
                let side = match t.trade_type.as_str() {
                    "ask" => Some("sell".into()),
                    "bid" => Some("buy".into()),
                    _ => None,
                };

                Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp: Some(t.timestamp * 1000),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.timestamp * 1000)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side,
                    taker_or_maker: None,
                    price: t.price,
                    amount: t.amount,
                    cost: Some(t.price * t.amount),
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
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetchOHLCV".into(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();
        let response: YobitBalanceInfo = self
            .private_post("getInfo", params)
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
        if order_type != OrderType::Limit {
            return Err(CcxtError::InvalidOrder {
                message: "Yobit only supports limit orders".into(),
            });
        }

        let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Limit order requires price".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("type".into(), match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("rate".into(), price_val.to_string());

        let response: YobitTradeResponse = self
            .private_post("Trade", params)
            .await?;

        let order = Order {
            id: response.order_id.to_string(),
            client_order_id: None,
            timestamp: Some(response.server_time * 1000),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.server_time * 1000)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: if response.remains > Decimal::ZERO {
                OrderStatus::Open
            } else {
                OrderStatus::Closed
            },
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price: Some(price_val),
            average: None,
            amount,
            filled: response.received,
            remaining: Some(response.remains),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(price_val * response.received),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        };

        Ok(order)
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: YobitCancelOrderResponse = self
            .private_post("CancelOrder", params)
            .await?;

        let order = Order {
            id: response.order_id.to_string(),
            client_order_id: None,
            timestamp: Some(response.server_time * 1000),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.server_time * 1000)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
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
        };

        Ok(order)
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let response: HashMap<String, YobitOrderInfo> = self
            .private_post("OrderInfo", params)
            .await?;

        let order_info = response.get(id).ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        // We need to get the symbol from the order info
        let symbol = order_info.pair.as_ref()
            .map(|p| self.to_symbol(p, &p.split('_').nth(0).unwrap_or(""), &p.split('_').nth(1).unwrap_or("")))
            .unwrap_or_default();

        Ok(self.parse_order(order_info, &symbol, Some(id)))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Yobit fetchOpenOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        let response: HashMap<String, YobitOrderInfo> = self
            .private_post("ActiveOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|(order_id, order_info)| self.parse_order(order_info, symbol, Some(order_id)))
            .collect();

        Ok(orders)
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
        _method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();
        let mut body = None;

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let nonce = self.next_nonce();
            let mut body_params = params.clone();
            body_params.insert("nonce".into(), nonce.to_string());

            let body_str: String = body_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(body_str.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
            headers.insert("Key".into(), api_key.to_string());
            headers.insert("Sign".into(), signature);

            body = Some(body_str);
            url = format!("{}/tapi", Self::BASE_URL);
        } else if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, query);
        }

        SignedRequest {
            url,
            method: "POST".to_string(),
            headers,
            body,
        }
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Yobit fetchMyTrades requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);

        if let Some(s) = since {
            params.insert("since".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("count".into(), l.to_string());
        }

        let response: HashMap<String, YobitTrade> = self
            .private_post("TradeHistory", params)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|(trade_id, trade_data)| self.parse_trade(trade_data, symbol, Some(trade_id)))
            .collect();

        Ok(trades)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("coinName".into(), code.to_lowercase());
        params.insert("need_new".into(), "0".to_string());

        let response: YobitDepositAddress = self
            .private_post("GetDepositAddress", params)
            .await?;

        Ok(crate::types::DepositAddress::new(
            code.to_string(),
            response.address,
        ))
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        _tag: Option<&str>,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::Transaction> {
        let mut params = HashMap::new();
        params.insert("coinName".into(), code.to_lowercase());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        let _response: serde_json::Value = self
            .private_post("WithdrawCoinsToAddress", params)
            .await?;

        Ok(crate::types::Transaction {
            id: String::new(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            updated: None,
            tx_type: crate::types::TransactionType::Withdrawal,
            currency: code.to_string(),
            network: None,
            amount,
            status: crate::types::TransactionStatus::Pending,
            address: Some(address.to_string()),
            tag: None,
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::Value::Null,
        })
    }
}

// === Yobit API Response Types ===

#[derive(Debug, Deserialize)]
struct YobitResponse<T> {
    success: i32,
    #[serde(rename = "return")]
    return_value: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YobitInfoResponse {
    server_time: i64,
    pairs: HashMap<String, YobitMarketInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitMarketInfo {
    decimal_places: i32,
    min_price: Decimal,
    max_price: Decimal,
    min_amount: Decimal,
    #[serde(default)]
    max_amount: Option<Decimal>,
    min_total: Decimal,
    hidden: i32,
    fee: f64,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitTickerData {
    high: Option<Decimal>,
    low: Option<Decimal>,
    avg: Option<Decimal>,
    vol: Option<Decimal>,
    vol_cur: Option<Decimal>,
    last: Option<Decimal>,
    buy: Option<Decimal>,
    sell: Option<Decimal>,
    updated: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct YobitOrderBookData {
    bids: Vec<Vec<Decimal>>,
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitPublicTrade {
    #[serde(rename = "type")]
    trade_type: String,
    price: Decimal,
    amount: Decimal,
    tid: i64,
    timestamp: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitTrade {
    pair: Option<String>,
    #[serde(rename = "type")]
    trade_type: Option<String>,
    amount: Option<String>,
    rate: Option<String>,
    price: Option<String>,
    order_id: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct YobitBalanceInfo {
    funds: Option<HashMap<String, String>>,
    funds_incl_orders: Option<HashMap<String, String>>,
    #[serde(default)]
    server_time: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitOrderInfo {
    pair: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    start_amount: Option<String>,
    amount: Option<String>,
    rate: Option<String>,
    timestamp_created: Option<i64>,
    status: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitTradeResponse {
    received: Decimal,
    remains: Decimal,
    order_id: i64,
    server_time: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct YobitCancelOrderResponse {
    order_id: i64,
    server_time: i64,
}

#[derive(Debug, Deserialize)]
struct YobitDepositAddress {
    address: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let yobit = Yobit::new(config).unwrap();

        assert_eq!(yobit.to_market_id("BTC/USDT"), "btc_usdt");
        assert_eq!(yobit.to_market_id("ETH/BTC"), "eth_btc");
        assert_eq!(yobit.to_symbol("btc_usdt", "btc", "usdt"), "BTC/USDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let yobit = Yobit::new(config).unwrap();

        assert_eq!(yobit.id(), ExchangeId::Yobit);
        assert_eq!(yobit.name(), "YoBit");
        assert!(yobit.has().spot);
        assert!(!yobit.has().margin);
        assert!(!yobit.has().swap);
        assert!(!yobit.has().create_market_order);
    }
}
