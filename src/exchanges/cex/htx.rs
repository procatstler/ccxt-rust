//! HTX (Huobi) Exchange Implementation
//!
//! HTX API 구현

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
    Fee, Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Transaction,
    TransactionStatus, TransactionType, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// HTX (Huobi) 거래소
pub struct Htx {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    account_id: RwLock<Option<String>>,
}

impl Htx {
    const BASE_URL: &'static str = "https://api.huobi.pro";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 HTX 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
            swap: true,
            future: true,
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
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/76137448-22748a80-6034-11ea-8ea6-e02dc4dbadc2.jpg".into()),
            api: api_urls,
            www: Some("https://www.htx.com".into()),
            doc: vec!["https://huobiapi.github.io/docs/spot/v1/en/".into()],
            fees: Some("https://www.htx.com/fee/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "60min".into());
        timeframes.insert(Timeframe::Hour4, "4hour".into());
        timeframes.insert(Timeframe::Day1, "1day".into());
        timeframes.insert(Timeframe::Week1, "1week".into());
        timeframes.insert(Timeframe::Month1, "1mon".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            account_id: RwLock::new(None),
        })
    }

    /// 심볼 변환 (BTC/USDT -> btcusdt)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "").to_lowercase()
    }

    /// 마켓 ID를 심볼로 변환 (btcusdt -> BTC/USDT)
    fn to_symbol(&self, market_id: &str) -> String {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned().unwrap_or_else(|| market_id.to_uppercase())
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

    /// 비공개 API 호출
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

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        let mut sign_params = HashMap::new();
        sign_params.insert("AccessKeyId".to_string(), api_key.to_string());
        sign_params.insert("SignatureMethod".to_string(), "HmacSHA256".to_string());
        sign_params.insert("SignatureVersion".to_string(), "2".to_string());
        sign_params.insert("Timestamp".to_string(), timestamp);

        // Sort and encode parameters
        let mut sorted_params: Vec<_> = sign_params.iter().collect();
        sorted_params.sort_by(|a, b| a.0.cmp(b.0));

        let query_string: String = sorted_params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let host = "api.huobi.pro";
        let payload = format!("{method}\n{host}\n{path}\n{query_string}");

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let signature = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            mac.finalize().into_bytes(),
        );

        sign_params.insert("Signature".to_string(), signature);

        let url = format!("{}?{}", path,
            sign_params.iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        );

        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        if method == "GET" {
            self.client.get(&url, None, Some(headers)).await
        } else {
            let body = serde_json::to_value(&params).ok();
            self.client.post(&url, body, Some(headers)).await
        }
    }

    /// Account ID 가져오기
    async fn get_account_id(&self) -> CcxtResult<String> {
        {
            let account_id = self.account_id.read().unwrap();
            if let Some(id) = account_id.as_ref() {
                return Ok(id.clone());
            }
        }

        let response: HtxResponse<Vec<HtxAccount>> = self.private_request(
            "GET",
            "/v1/account/accounts",
            HashMap::new(),
        ).await?;

        let accounts = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to get accounts".into(),
        })?;

        let account = accounts.into_iter()
            .find(|a| a.account_type == "spot")
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Spot account not found".into(),
            })?;

        let id = account.id.to_string();
        {
            let mut account_id = self.account_id.write().unwrap();
            *account_id = Some(id.clone());
        }
        Ok(id)
    }

    /// Timeframe 변환
    fn get_timeframe(&self, timeframe: Timeframe) -> Option<&String> {
        self.timeframes.get(&timeframe)
    }
}

#[async_trait]
impl Exchange for Htx {
    fn id(&self) -> ExchangeId {
        ExchangeId::Htx
    }

    fn name(&self) -> &str {
        "HTX"
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

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        let response: HtxResponse<Vec<HtxSymbol>> = self.public_get("/v1/common/symbols", None).await?;

        let symbols = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Failed to load markets".into(),
        })?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for symbol_data in symbols {
            if symbol_data.state != "online" {
                continue;
            }

            let base = symbol_data.base_currency.to_uppercase();
            let quote = symbol_data.quote_currency.to_uppercase();
            let symbol = format!("{base}/{quote}");
            let market_id = symbol_data.symbol.clone();

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.clone()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_data.base_currency.clone(),
                quote_id: symbol_data.quote_currency.clone(),
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
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                taker: Some(Decimal::new(2, 3)), // 0.2%
                maker: Some(Decimal::new(2, 3)), // 0.2%
                percentage: true,
                tier_based: false,
                precision: MarketPrecision {
                    amount: Some(symbol_data.amount_precision),
                    price: Some(symbol_data.price_precision),
                    cost: None,
                    base: Some(symbol_data.amount_precision),
                    quote: Some(symbol_data.price_precision),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: symbol_data.min_order_amt.as_ref().and_then(|v| v.parse().ok()),
                        max: symbol_data.max_order_amt.as_ref().and_then(|v| v.parse().ok()),
                    },
                    price: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                    cost: crate::types::MinMax {
                        min: symbol_data.min_order_value.as_ref().and_then(|v| v.parse().ok()),
                        max: None,
                    },
                    leverage: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_data).unwrap_or_default(),
            };

            markets_by_id.insert(market_id, symbol.clone());
            markets.insert(symbol, market);
        }

        *self.markets.write().unwrap() = markets.clone();
        *self.markets_by_id.write().unwrap() = markets_by_id;

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: HtxResponse<HtxTickerWrapper> = self.public_get("/market/detail/merged", Some(params)).await?;

        let ticker_data = response.tick.ok_or_else(|| CcxtError::ExchangeError {
            message: "Ticker data not found".into(),
        })?;

        let timestamp = response.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker_data.high,
            low: ticker_data.low,
            bid: ticker_data.bid.first().copied(),
            bid_volume: ticker_data.bid.get(1).copied(),
            ask: ticker_data.ask.first().copied(),
            ask_volume: ticker_data.ask.get(1).copied(),
            vwap: None,
            open: ticker_data.open,
            close: ticker_data.close,
            last: ticker_data.close,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: ticker_data.amount,
            quote_volume: ticker_data.vol,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&ticker_data).unwrap_or_default(),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: HtxResponse<Vec<HtxAllTicker>> = self.public_get("/market/tickers", None).await?;

        let tickers_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Tickers data not found".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let mut tickers = HashMap::new();

        for ticker_data in tickers_data {
            let symbol = self.to_symbol(&ticker_data.symbol);

            if let Some(filter_symbols) = symbols {
                if !filter_symbols.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                high: ticker_data.high,
                low: ticker_data.low,
                bid: ticker_data.bid,
                bid_volume: ticker_data.bid_size,
                ask: ticker_data.ask,
                ask_volume: ticker_data.ask_size,
                vwap: None,
                open: ticker_data.open,
                close: ticker_data.close,
                last: ticker_data.close,
                previous_close: None,
                change: None,
                percentage: None,
                average: None,
                base_volume: ticker_data.amount,
                quote_volume: ticker_data.vol,
                index_price: None,
                mark_price: None,
                info: serde_json::to_value(&ticker_data).unwrap_or_default(),
            };

            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let depth = limit.unwrap_or(20).min(150);
        let depth_type = if depth <= 5 { "step0" } else if depth <= 20 { "step1" } else { "step2" };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("type".to_string(), depth_type.to_string());

        let response: HtxResponse<HtxOrderBook> = self.public_get("/market/depth", Some(params)).await?;

        let ob_data = response.tick.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order book data not found".into(),
        })?;

        let timestamp = response.ts.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = ob_data.bids.iter().filter_map(|b| {
            if b.len() >= 2 {
                Some(OrderBookEntry {
                    price: b[0],
                    amount: b[1],
                })
            } else {
                None
            }
        }).collect();

        let asks: Vec<OrderBookEntry> = ob_data.asks.iter().filter_map(|a| {
            if a.len() >= 2 {
                Some(OrderBookEntry {
                    price: a[0],
                    amount: a[1],
                })
            } else {
                None
            }
        }).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: ob_data.version,
            bids,
            asks,
        })
    }

    async fn fetch_trades(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("size".to_string(), limit.unwrap_or(100).min(2000).to_string());

        let response: HtxResponse<Vec<HtxTradeWrapper>> = self.public_get("/market/history/trade", Some(params)).await?;

        let trade_wrappers = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Trades data not found".into(),
        })?;

        let mut trades = Vec::new();

        for wrapper in trade_wrappers {
            for trade_data in wrapper.data {
                let timestamp = trade_data.ts;

                if let Some(since_ts) = since {
                    if timestamp < since_ts {
                        continue;
                    }
                }

                let trade = Trade {
                    id: trade_data.trade_id.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(trade_data.direction.clone()),
                    taker_or_maker: None,
                    price: trade_data.price,
                    amount: trade_data.amount,
                    cost: Some(trade_data.price * trade_data.amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(&trade_data).unwrap_or_default(),
                };

                trades.push(trade);
            }
        }

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
        let period = self.get_timeframe(timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("period".to_string(), period.clone());
        params.insert("size".to_string(), limit.unwrap_or(200).min(2000).to_string());

        let response: HtxResponse<Vec<HtxCandle>> = self.public_get("/market/history/kline", Some(params)).await?;

        let candles = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "OHLCV data not found".into(),
        })?;

        let mut ohlcv_list = Vec::new();

        for candle in candles {
            let timestamp = candle.id * 1000;

            if let Some(since_ts) = since {
                if timestamp < since_ts {
                    continue;
                }
            }

            ohlcv_list.push(OHLCV::new(
                timestamp,
                candle.open,
                candle.high,
                candle.low,
                candle.close,
                candle.amount,
            ));
        }

        Ok(ohlcv_list)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let account_id = self.get_account_id().await?;
        let path = format!("/v1/account/accounts/{account_id}/balance");

        let response: HtxResponse<HtxBalance> = self.private_request("GET", &path, HashMap::new()).await?;

        let balance_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Balance data not found".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let mut currencies: HashMap<String, Balance> = HashMap::new();

        for item in balance_data.list {
            let currency = item.currency.to_uppercase();
            let amount: Decimal = item.balance.parse().unwrap_or_default();

            let entry = currencies.entry(currency.clone()).or_insert_with(|| Balance {
                free: Some(Decimal::ZERO),
                used: Some(Decimal::ZERO),
                total: Some(Decimal::ZERO),
                debt: None,
            });

            match item.balance_type.as_str() {
                "trade" => entry.free = Some(amount),
                "frozen" => entry.used = Some(amount),
                _ => {}
            }
            let free = entry.free.unwrap_or_default();
            let used = entry.used.unwrap_or_default();
            entry.total = Some(free + used);
        }

        Ok(Balances {
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            currencies,
            info: serde_json::json!({}),
        })
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let account_id = self.get_account_id().await?;
        let market_id = self.to_market_id(symbol);

        let order_type_str = match (order_type, side) {
            (OrderType::Limit, OrderSide::Buy) => "buy-limit",
            (OrderType::Limit, OrderSide::Sell) => "sell-limit",
            (OrderType::Market, OrderSide::Buy) => "buy-market",
            (OrderType::Market, OrderSide::Sell) => "sell-market",
            _ => return Err(CcxtError::BadRequest {
                message: "Unsupported order type".into(),
            }),
        };

        let mut request_params = HashMap::new();
        request_params.insert("account-id".to_string(), account_id);
        request_params.insert("symbol".to_string(), market_id);
        request_params.insert("type".to_string(), order_type_str.to_string());
        request_params.insert("amount".to_string(), amount.to_string());

        if let Some(p) = price {
            request_params.insert("price".to_string(), p.to_string());
        }

        let response: HtxResponse<String> = self.private_request("POST", "/v1/order/orders/place", request_params).await?;

        let order_id = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order creation failed".into(),
        })?;

        self.fetch_order(&order_id, symbol).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/order/orders/{id}/submitcancel");
        let _response: HtxResponse<String> = self.private_request("POST", &path, HashMap::new()).await?;

        self.fetch_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/v1/order/orders/{id}");
        let response: HtxResponse<HtxOrder> = self.private_request("GET", &path, HashMap::new()).await?;

        let order_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Order not found".into(),
        })?;

        self.parse_order(&order_data)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol is required".into(),
        })?;

        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("states".to_string(), "pre-submitted,submitted,partial-filled".to_string());

        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string());
        }

        let response: HtxResponse<Vec<HtxOrder>> = self.private_request("GET", "/v1/order/orders", params).await?;

        let orders_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Orders not found".into(),
        })?;

        let mut orders = Vec::new();
        for order_data in orders_data {
            if let Some(since_ts) = since {
                if order_data.created_at < since_ts {
                    continue;
                }
            }
            orders.push(self.parse_order(&order_data)?);
        }

        Ok(orders)
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol is required".into(),
        })?;

        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("states".to_string(), "filled,partial-canceled,canceled".to_string());

        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string());
        }

        let response: HtxResponse<Vec<HtxOrder>> = self.private_request("GET", "/v1/order/orders", params).await?;

        let orders_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Orders not found".into(),
        })?;

        let mut orders = Vec::new();
        for order_data in orders_data {
            if let Some(since_ts) = since {
                if order_data.created_at < since_ts {
                    continue;
                }
            }
            orders.push(self.parse_order(&order_data)?);
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol is required".into(),
        })?;

        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string());
        }

        let response: HtxResponse<Vec<HtxMyTrade>> = self.private_request("GET", "/v1/order/matchresults", params).await?;

        let trades_data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "Trades not found".into(),
        })?;

        let mut trades = Vec::new();
        for trade_data in trades_data {
            if let Some(since_ts) = since {
                if trade_data.created_at < since_ts {
                    continue;
                }
            }

            let price: Decimal = trade_data.price.parse().unwrap_or_default();
            let amount: Decimal = trade_data.filled_amount.parse().unwrap_or_default();
            let fee_amount: Decimal = trade_data.filled_fees.parse().unwrap_or_default();

            let trade = Trade {
                id: trade_data.id.to_string(),
                order: Some(trade_data.order_id.to_string()),
                timestamp: Some(trade_data.created_at),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(trade_data.created_at)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(trade_data.trade_type.split('-').next().unwrap_or("").to_string()),
                taker_or_maker: Some(if trade_data.role == "maker" {
                    crate::types::TakerOrMaker::Maker
                } else {
                    crate::types::TakerOrMaker::Taker
                }),
                price,
                amount,
                cost: Some(price * amount),
                fee: Some(Fee {
                    cost: Some(fee_amount),
                    currency: Some(trade_data.fee_currency.to_uppercase()),
                    rate: None,
                }),
                fees: Vec::new(),
                info: serde_json::to_value(&trade_data).unwrap_or_default(),
            };

            trades.push(trade);
        }

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        params.insert("type".to_string(), "deposit".to_string());

        if let Some(currency) = code {
            params.insert("currency".to_string(), currency.to_lowercase());
        }
        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string());
        }

        let response: HtxResponse<Vec<HtxTransaction>> = self.private_request("GET", "/v1/query/deposit-withdraw", params).await?;

        let tx_data = response.data.unwrap_or_default();

        let mut transactions = Vec::new();
        for tx in tx_data {
            if let Some(since_ts) = since {
                if tx.created_at < since_ts {
                    continue;
                }
            }

            transactions.push(self.parse_transaction(&tx, TransactionType::Deposit));
        }

        Ok(transactions)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        params.insert("type".to_string(), "withdraw".to_string());

        if let Some(currency) = code {
            params.insert("currency".to_string(), currency.to_lowercase());
        }
        if let Some(l) = limit {
            params.insert("size".to_string(), l.to_string());
        }

        let response: HtxResponse<Vec<HtxTransaction>> = self.private_request("GET", "/v1/query/deposit-withdraw", params).await?;

        let tx_data = response.data.unwrap_or_default();

        let mut transactions = Vec::new();
        for tx in tx_data {
            if let Some(since_ts) = since {
                if tx.created_at < since_ts {
                    continue;
                }
            }

            transactions.push(self.parse_transaction(&tx, TransactionType::Withdrawal));
        }

        Ok(transactions)
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_lowercase());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("addr-tag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: HtxResponse<i64> = self
            .private_request("POST", "/v1/dw/withdraw/api/create", params)
            .await?;

        let withdraw_id = response.data.unwrap_or(0);
        let timestamp = Utc::now().timestamp_millis();

        Ok(Transaction {
            id: withdraw_id.to_string(),
            txid: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            address: Some(address.to_string()),
            tag: tag.map(|t| t.to_string()),
            tx_type: TransactionType::Withdrawal,
            amount,
            currency: code.to_string(),
            status: TransactionStatus::Pending,
            fee: None,
            network: network.map(|n| n.to_string()),
            info: serde_json::json!({"id": withdraw_id}),
            updated: None,
            internal: None,
            confirmations: None,
        })
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_lowercase());

        let response: HtxResponse<Vec<HtxDepositAddress>> = self
            .private_request("GET", "/v2/account/deposit/address", params)
            .await?;

        let addresses = response.data.unwrap_or_default();

        // Find the address matching the network, or use the first one
        let address_data = if let Some(n) = network {
            addresses.iter().find(|a| a.chain.eq_ignore_ascii_case(n))
        } else {
            addresses.first()
        };

        let addr = address_data.ok_or_else(|| CcxtError::BadResponse {
            message: format!("No deposit address found for {code}"),
        })?;

        Ok(DepositAddress {
            currency: code.to_string(),
            address: addr.address.clone(),
            tag: addr.address_tag.clone(),
            network: Some(addr.chain.clone()),
            info: serde_json::to_value(addr).unwrap_or_default(),
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
        _api: &str,
        method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("{}{}", Self::BASE_URL, path);
        SignedRequest {
            url,
            method: method.to_string(),
            headers: HashMap::new(),
            body: body.map(|s| s.to_string()),
        }
    }
}

impl Htx {
    fn parse_order(&self, order_data: &HtxOrder) -> CcxtResult<Order> {
        let symbol = self.to_symbol(&order_data.symbol);
        let timestamp = order_data.created_at;

        let (order_type, side) = match order_data.order_type.as_str() {
            "buy-limit" => (OrderType::Limit, OrderSide::Buy),
            "sell-limit" => (OrderType::Limit, OrderSide::Sell),
            "buy-market" => (OrderType::Market, OrderSide::Buy),
            "sell-market" => (OrderType::Market, OrderSide::Sell),
            "buy-limit-maker" => (OrderType::Limit, OrderSide::Buy),
            "sell-limit-maker" => (OrderType::Limit, OrderSide::Sell),
            "buy-ioc" => (OrderType::Limit, OrderSide::Buy),
            "sell-ioc" => (OrderType::Limit, OrderSide::Sell),
            _ => (OrderType::Limit, OrderSide::Buy),
        };

        let status = match order_data.state.as_str() {
            "submitted" | "pre-submitted" => OrderStatus::Open,
            "partial-filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "partial-canceled" | "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let price: Decimal = order_data.price.parse().unwrap_or_default();
        let amount: Decimal = order_data.amount.parse().unwrap_or_default();
        let filled: Decimal = order_data.field_amount.parse().unwrap_or_default();
        let cost: Decimal = order_data.field_cash_amount.parse().unwrap_or_default();
        let fee: Decimal = order_data.field_fees.parse().unwrap_or_default();

        Ok(Order {
            id: order_data.id.to_string(),
            client_order_id: order_data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: if filled > Decimal::ZERO { Some(cost / filled) } else { None },
            amount,
            filled,
            remaining: Some(amount - filled),
            cost: Some(cost),
            trades: Vec::new(),
            fee: Some(Fee {
                cost: Some(fee),
                currency: None,
                rate: None,
            }),
            fees: Vec::new(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(order_data).unwrap_or_default(),
        })
    }

    fn parse_transaction(&self, tx: &HtxTransaction, tx_type: TransactionType) -> Transaction {
        let status = match tx.state.as_str() {
            "safe" | "confirmed" => TransactionStatus::Ok,
            "orphan" | "unknown" | "confirming" => TransactionStatus::Pending,
            "wallet-reject" | "confirm-error" | "repealed" => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        Transaction {
            id: tx.id.to_string(),
            txid: tx.tx_hash.clone(),
            timestamp: Some(tx.created_at),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(tx.created_at)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            network: tx.chain.clone(),
            address: tx.address.clone(),
            tag: tx.address_tag.clone(),
            tx_type,
            amount: tx.amount.parse().unwrap_or_default(),
            currency: tx.currency.to_uppercase(),
            status,
            updated: tx.updated_at,
            internal: None,
            confirmations: None,
            fee: tx.fee.as_ref().and_then(|f| f.parse().ok()).map(|cost| Fee {
                cost: Some(cost),
                currency: Some(tx.currency.to_uppercase()),
                rate: None,
            }),
            info: serde_json::to_value(tx).unwrap_or_default(),
        }
    }
}

// === HTX API Response Types ===

#[derive(Debug, Deserialize)]
struct HtxResponse<T> {
    status: String,
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    tick: Option<T>,
    #[serde(default)]
    ts: Option<i64>,
    #[serde(rename = "err-msg", default)]
    err_msg: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct HtxSymbol {
    symbol: String,
    base_currency: String,
    quote_currency: String,
    state: String,
    #[serde(default)]
    amount_precision: i32,
    #[serde(default)]
    price_precision: i32,
    #[serde(default)]
    min_order_amt: Option<String>,
    #[serde(default)]
    max_order_amt: Option<String>,
    #[serde(default)]
    min_order_value: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HtxTickerWrapper {
    #[serde(default)]
    bid: Vec<Decimal>,
    #[serde(default)]
    ask: Vec<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HtxAllTicker {
    symbol: String,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(rename = "bidSize", default)]
    bid_size: Option<Decimal>,
    #[serde(rename = "askSize", default)]
    ask_size: Option<Decimal>,
}

#[derive(Debug, Default, Deserialize)]
struct HtxOrderBook {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
    #[serde(default)]
    version: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct HtxTradeWrapper {
    #[serde(default)]
    data: Vec<HtxTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct HtxTrade {
    trade_id: i64,
    price: Decimal,
    amount: Decimal,
    direction: String,
    ts: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct HtxCandle {
    id: i64,
    open: Decimal,
    close: Decimal,
    high: Decimal,
    low: Decimal,
    amount: Decimal,
    #[serde(default)]
    vol: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct HtxAccount {
    id: i64,
    account_type: String,
    state: String,
}

#[derive(Debug, Default, Deserialize)]
struct HtxBalance {
    #[serde(default)]
    list: Vec<HtxBalanceItem>,
}

#[derive(Debug, Deserialize)]
struct HtxBalanceItem {
    currency: String,
    #[serde(rename = "type")]
    balance_type: String,
    balance: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct HtxOrder {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    symbol: String,
    #[serde(rename = "type", default)]
    order_type: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    field_amount: String,
    #[serde(default)]
    field_cash_amount: String,
    #[serde(default)]
    field_fees: String,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    client_order_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct HtxMyTrade {
    id: i64,
    order_id: i64,
    symbol: String,
    #[serde(rename = "type")]
    trade_type: String,
    price: String,
    filled_amount: String,
    filled_fees: String,
    fee_currency: String,
    role: String,
    created_at: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
struct HtxTransaction {
    id: i64,
    #[serde(rename = "type")]
    tx_type: String,
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    amount: String,
    state: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_tag: Option<String>,
    #[serde(default)]
    tx_hash: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    created_at: i64,
    #[serde(default)]
    updated_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HtxDepositAddress {
    #[serde(default)]
    address: String,
    #[serde(default)]
    address_tag: Option<String>,
    #[serde(default)]
    chain: String,
    #[serde(default)]
    currency: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Htx::new(config).unwrap();

        assert_eq!(exchange.name(), "HTX");
        assert!(exchange.has().spot);
        assert!(exchange.has().fetch_ticker);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::default();
        let exchange = Htx::new(config).unwrap();

        assert_eq!(exchange.to_market_id("BTC/USDT"), "btcusdt");
        assert_eq!(exchange.to_market_id("ETH/BTC"), "ethbtc");
    }
}
