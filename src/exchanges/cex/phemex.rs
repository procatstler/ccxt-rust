//! Phemex Exchange Implementation
//!
//! Phemex 거래소 구현체

use async_trait::async_trait;
use chrono::Utc;
use hmac::Hmac;
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
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// Phemex 거래소
pub struct Phemex {
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

impl Phemex {
    const BASE_URL: &'static str = "https://api.phemex.com";
    const RATE_LIMIT_MS: u64 = 50; // 500 requests per 5 seconds

    /// 새 Phemex 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
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
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            ws: true,
            watch_ticker: true,
            watch_tickers: true,
            watch_order_book: true,
            watch_trades: true,
            watch_ohlcv: true,
            watch_balance: true,
            watch_orders: true,
            watch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/85225056-221eb600-b3d7-11ea-930d-564d2690e3f6.jpg".into()),
            api: api_urls,
            www: Some("https://phemex.com".into()),
            doc: vec![
                "https://github.com/phemex/phemex-api-docs".into(),
            ],
            fees: Some("https://phemex.com/fees-conditions".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".into());
        timeframes.insert(Timeframe::Minute5, "300".into());
        timeframes.insert(Timeframe::Minute15, "900".into());
        timeframes.insert(Timeframe::Minute30, "1800".into());
        timeframes.insert(Timeframe::Hour1, "3600".into());
        timeframes.insert(Timeframe::Hour4, "14400".into());
        timeframes.insert(Timeframe::Day1, "86400".into());
        timeframes.insert(Timeframe::Week1, "604800".into());
        timeframes.insert(Timeframe::Month1, "2592000".into());

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
        self.public_client.get(path, params, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        use hmac::Mac;

        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let expiry = (Utc::now().timestamp() + 60).to_string();

        // Build query string for GET, body for POST
        let (query_string, body) = if method == "GET" {
            let qs = params
                .as_ref()
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                        .collect::<Vec<_>>()
                        .join("&")
                })
                .unwrap_or_default();
            (qs, None)
        } else {
            let b = params
                .as_ref()
                .map(|p| serde_json::to_string(p).unwrap_or_default());
            (String::new(), b)
        };

        // Sign: path + queryString + expiry + body
        let sign_str = format!(
            "{}{}{}{}",
            path,
            query_string,
            expiry,
            body.as_deref().unwrap_or("")
        );

        let signature = {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(sign_str.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };

        let mut headers = HashMap::new();
        headers.insert("x-phemex-access-token".into(), api_key.to_string());
        headers.insert("x-phemex-request-expiry".into(), expiry);
        headers.insert("x-phemex-request-signature".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => {
                let url = if query_string.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}?{query_string}")
                };
                self.private_client.get(&url, None, Some(headers)).await
            },
            "POST" => {
                let json_body = params.map(|p| serde_json::to_value(p).unwrap_or_default());
                self.private_client
                    .post(path, json_body, Some(headers))
                    .await
            },
            "DELETE" => {
                let url = if query_string.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}?{query_string}")
                };
                self.private_client.delete(&url, None, Some(headers)).await
            },
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 파싱
    fn parse_market(&self, data: &PhemexProduct) -> Market {
        let base = data.base_currency.clone().unwrap_or_default();
        let quote = data.quote_currency.clone().unwrap_or_default();
        let symbol = format!("{base}/{quote}");

        Market {
            id: data.symbol.clone(),
            lowercase_id: Some(data.symbol.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base.clone(),
            quote_id: quote.clone(),
            market_type: if data.product_type.as_deref() == Some("Perpetual") {
                MarketType::Swap
            } else {
                MarketType::Spot
            },
            spot: data.product_type.as_deref() == Some("Spot"),
            margin: false,
            swap: data.product_type.as_deref() == Some("Perpetual"),
            future: false,
            option: false,
            index: false,
            active: data.status.as_deref() == Some("Listed"),
            contract: data.product_type.as_deref() == Some("Perpetual"),
            linear: Some(true),
            inverse: Some(false),
            sub_type: None,
            taker: data.taker_fee_rate_er.map(|r| Decimal::new(r, 8)),
            maker: data.maker_fee_rate_er.map(|r| Decimal::new(r, 8)),
            contract_size: data.contract_size.map(|s| Decimal::new(s, 0)),
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            underlying: None,
            underlying_id: None,
            settle: data.settle_currency.clone(),
            settle_id: data.settle_currency.clone(),
            precision: MarketPrecision {
                amount: data.lot_size.map(|_| 8),
                price: data.tick_size.map(|_| 8),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits::default(),
            margin_modes: None,
            created: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            tier_based: false,
            percentage: true,
        }
    }

    /// 티커 파싱
    fn parse_ticker(&self, data: &PhemexTicker, symbol: &str) -> Ticker {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        // Phemex uses scaled integers, need to divide by scale factor
        let scale = Decimal::new(1, 8); // 10^-8

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_ep.map(|v| Decimal::new(v, 0) * scale),
            low: data.low_ep.map(|v| Decimal::new(v, 0) * scale),
            bid: data.bid_ep.map(|v| Decimal::new(v, 0) * scale),
            bid_volume: None,
            ask: data.ask_ep.map(|v| Decimal::new(v, 0) * scale),
            ask_volume: None,
            vwap: None,
            open: data.open_ep.map(|v| Decimal::new(v, 0) * scale),
            close: data.last_ep.map(|v| Decimal::new(v, 0) * scale),
            last: data.last_ep.map(|v| Decimal::new(v, 0) * scale),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume_ev.map(|v| Decimal::new(v, 0) * scale),
            quote_volume: data.turnover_ev.map(|v| Decimal::new(v, 0) * scale),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, data: &PhemexOrder) -> Order {
        let timestamp = data
            .create_time_ns
            .map(|t| t / 1_000_000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let scale = Decimal::new(1, 8);

        let status = match data.ord_status.as_deref() {
            Some("New") | Some("PartiallyFilled") => OrderStatus::Open,
            Some("Filled") => OrderStatus::Closed,
            Some("Canceled") | Some("Rejected") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("Buy") => OrderSide::Buy,
            Some("Sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.ord_type.as_deref() {
            Some("Limit") => OrderType::Limit,
            Some("Market") => OrderType::Market,
            Some("Stop") => OrderType::StopMarket,
            Some("StopLimit") => OrderType::StopLimit,
            _ => OrderType::Limit,
        };

        let price = data.price_ep.map(|p| Decimal::new(p, 0) * scale);
        let amount = data
            .order_qty
            .map(|q| Decimal::new(q, 0) * scale)
            .unwrap_or_default();
        let filled = data
            .cum_qty
            .map(|q| Decimal::new(q, 0) * scale)
            .unwrap_or_default();
        let remaining = amount - filled;
        let cost = data.cum_value_ev.map(|v| Decimal::new(v, 0) * scale);

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.cl_ord_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.action_time_ns.map(|t| t / 1_000_000),
            status,
            symbol: data.symbol.clone().unwrap_or_default(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: if filled > Decimal::ZERO {
                cost.map(|c| c / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: data.stop_px_ep.map(|p| Decimal::new(p, 0) * scale),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: data.reduce_only,
            post_only: data.post_only,
        }
    }

    /// 거래 파싱
    fn parse_trade(&self, data: &PhemexTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let scale = Decimal::new(1, 8);
        let price = data
            .price_ep
            .map(|p| Decimal::new(p, 0) * scale)
            .unwrap_or_default();
        let amount = data
            .qty
            .map(|q| Decimal::new(q, 0) * scale)
            .unwrap_or_default();

        let side = match data.side.as_deref() {
            Some("Buy") => "buy",
            Some("Sell") => "sell",
            _ => "buy",
        };

        Trade {
            id: data.trade_id.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 파싱
    fn parse_ohlcv(&self, data: &[i64]) -> Option<OHLCV> {
        if data.len() < 6 {
            return None;
        }
        let scale = Decimal::new(1, 8);
        Some(OHLCV {
            timestamp: data[0] * 1000, // seconds to ms
            open: Decimal::new(data[3], 0) * scale,
            high: Decimal::new(data[4], 0) * scale,
            low: Decimal::new(data[5], 0) * scale,
            close: Decimal::new(data[6], 0) * scale,
            volume: Decimal::new(data[7], 0) * scale,
        })
    }

    /// 심볼 변환 (BTC/USDT -> BTCUSDT)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }
}

#[async_trait]
impl Exchange for Phemex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Phemex
    }

    fn name(&self) -> &str {
        "Phemex"
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
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        use hmac::Mac;

        let api_key = self.config.api_key().unwrap_or("");
        let api_secret = self.config.secret().unwrap_or("");
        let expiry = (Utc::now().timestamp() + 60).to_string();

        let (query_string, body) = if method == "GET" {
            let qs = if params.is_empty() {
                String::new()
            } else {
                params
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&")
            };
            (qs, None)
        } else {
            let b = if params.is_empty() {
                None
            } else {
                Some(serde_json::to_string(params).unwrap_or_default())
            };
            (String::new(), b)
        };

        let sign_str = format!(
            "{}{}{}{}",
            path,
            query_string,
            expiry,
            body.as_deref().unwrap_or("")
        );

        let signature = if !api_secret.is_empty() {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(sign_str.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        } else {
            String::new()
        };

        let mut headers = HashMap::new();
        headers.insert("x-phemex-access-token".into(), api_key.to_string());
        headers.insert("x-phemex-request-expiry".into(), expiry);
        headers.insert("x-phemex-request-signature".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        let url = if method == "GET" && !query_string.is_empty() {
            format!("{path}?{query_string}")
        } else {
            path.to_string()
        };

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: PhemexProductsResponse = self.public_get("/public/products", None).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Markets".to_string(),
            message: "No data in response".into(),
        })?;

        let mut markets = Vec::new();
        for product in &data.products {
            markets.push(self.parse_market(product));
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
        params.insert("symbol".into(), market_id);

        let response: PhemexTickerResponse =
            self.public_get("/md/v2/ticker/24hr", Some(params)).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.result.ok_or(CcxtError::ParseError {
            data_type: "Ticker".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(self.parse_ticker(&data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: PhemexTickersResponse =
            self.public_get("/md/v2/ticker/24hr/all", None).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.result.ok_or(CcxtError::ParseError {
            data_type: "Tickers".to_string(),
            message: "No data in response".into(),
        })?;

        let mut result = HashMap::new();
        for ticker_data in &data {
            let sym = ticker_data.symbol.clone().unwrap_or_default();
            // Convert market ID to symbol
            let symbol_str = self.symbol(&sym).unwrap_or(sym.clone());

            if let Some(filter) = symbols {
                if filter.contains(&symbol_str.as_str()) {
                    let ticker = self.parse_ticker(ticker_data, &symbol_str);
                    result.insert(symbol_str, ticker);
                }
            } else {
                let ticker = self.parse_ticker(ticker_data, &symbol_str);
                result.insert(symbol_str, ticker);
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: PhemexOrderBookResponse =
            self.public_get("/md/v2/orderbook", Some(params)).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.result.ok_or(CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data in response".into(),
        })?;

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let scale = Decimal::new(1, 8);
        let limit_usize = limit.unwrap_or(100) as usize;

        let bids: Vec<OrderBookEntry> = data
            .book
            .bids
            .iter()
            .take(limit_usize)
            .filter_map(|b| {
                if b.len() >= 2 {
                    let price = Decimal::new(b[0], 0) * scale;
                    let amount = Decimal::new(b[1], 0) * scale;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .book
            .asks
            .iter()
            .take(limit_usize)
            .filter_map(|a| {
                if a.len() >= 2 {
                    let price = Decimal::new(a[0], 0) * scale;
                    let amount = Decimal::new(a[1], 0) * scale;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
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
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: PhemexTradesResponse = self.public_get("/md/v2/trade", Some(params)).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.result.ok_or(CcxtError::ParseError {
            data_type: "Trades".to_string(),
            message: "No data in response".into(),
        })?;

        let limit_usize = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = data
            .trades
            .iter()
            .take(limit_usize)
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
        let market_id = self.convert_to_market_id(symbol);
        let resolution = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or("3600".into());

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("resolution".into(), resolution);

        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: PhemexKlinesResponse = self.public_get("/md/v2/kline", Some(params)).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.result.ok_or(CcxtError::ParseError {
            data_type: "OHLCV".to_string(),
            message: "No data in response".into(),
        })?;

        let ohlcv: Vec<OHLCV> = data
            .rows
            .iter()
            .filter_map(|k| self.parse_ohlcv(k))
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: PhemexBalanceResponse =
            self.private_request("GET", "/spot/wallets", None).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Balance".to_string(),
            message: "No data in response".into(),
        })?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(
            chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );

        let scale = Decimal::new(1, 8);
        for wallet in data {
            let currency = wallet.currency.clone();
            let free = wallet
                .available_balance_ev
                .map(|v| Decimal::new(v, 0) * scale)
                .unwrap_or_default();
            let total = wallet
                .balance_ev
                .map(|v| Decimal::new(v, 0) * scale)
                .unwrap_or_default();
            let used = total - free;

            balances.currencies.insert(
                currency,
                Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(total),
                    debt: None,
                },
            );
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
        let scale = Decimal::new(10_i64.pow(8), 0); // Scale up to Phemex format

        let mut request = HashMap::new();
        request.insert("symbol".into(), market_id);
        request.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "Buy".into(),
                OrderSide::Sell => "Sell".into(),
            },
        );
        request.insert(
            "ordType".into(),
            match order_type {
                OrderType::Limit => "Limit".into(),
                OrderType::Market => "Market".into(),
                _ => "Limit".into(),
            },
        );

        let qty_scaled = (amount * scale).to_string();
        request.insert("orderQtyEv".into(), qty_scaled);

        if let Some(p) = price {
            let price_scaled = (p * scale).to_string();
            request.insert("priceEp".into(), price_scaled);
        }

        let response: PhemexOrderResponse = self
            .private_request("POST", "/spot/orders", Some(request))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Order".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(self.parse_order(&data))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderID".into(), id.to_string());

        let response: PhemexOrderResponse = self
            .private_request("DELETE", "/spot/orders", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderID".into(), id.to_string());

        let response: PhemexOrderResponse = self
            .private_request("GET", "/spot/orders/active", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Order".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(self.parse_order(&data))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".into(), self.convert_to_market_id(s));
        }

        let response: PhemexOrdersResponse = self
            .private_request(
                "GET",
                "/spot/orders",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.rows.iter().map(|o| self.parse_order(o)).collect();

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
            params.insert("symbol".into(), self.convert_to_market_id(s));
        }
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: PhemexMyTradesResponse = self
            .private_request(
                "GET",
                "/spot/orders/trades",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Trades".to_string(),
            message: "No data in response".into(),
        })?;

        let sym = symbol.unwrap_or("");
        let trades: Vec<Trade> = data.rows.iter().map(|t| self.parse_trade(t, sym)).collect();

        Ok(trades)
    }

    async fn create_limit_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
        price: Decimal,
    ) -> CcxtResult<Order> {
        self.create_order(symbol, OrderType::Limit, side, amount, Some(price))
            .await
    }

    async fn create_market_order(
        &self,
        symbol: &str,
        side: OrderSide,
        amount: Decimal,
    ) -> CcxtResult<Order> {
        self.create_order(symbol, OrderType::Market, side, amount, None)
            .await
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".into(), self.convert_to_market_id(s));
        }

        let _response: PhemexResponse<serde_json::Value> = self
            .private_request(
                "DELETE",
                "/spot/orders/all",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        // Phemex doesn't return cancelled orders in response, return empty vec
        Ok(Vec::new())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".into(), self.convert_to_market_id(s));
        }
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: PhemexOrdersResponse = self
            .private_request(
                "GET",
                "/spot/orders/closed",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.rows.iter().map(|o| self.parse_order(o)).collect();
        Ok(orders)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        // Combine open and closed orders
        let mut all_orders = Vec::new();

        if let Ok(open_orders) = self.fetch_open_orders(symbol, since, limit).await {
            all_orders.extend(open_orders);
        }

        if let Ok(closed_orders) = self.fetch_closed_orders(symbol, since, limit).await {
            all_orders.extend(closed_orders);
        }

        // Sort by timestamp descending
        all_orders.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        if let Some(l) = limit {
            all_orders.truncate(l as usize);
        }

        Ok(all_orders)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        let response: PhemexCurrenciesResponse =
            self.public_get("/public/cfg/currencies", None).await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let mut currencies = HashMap::new();

        for c in data {
            let currency = crate::types::Currency {
                id: c.currency.clone(),
                code: c.currency.clone(),
                name: c.name.clone(),
                active: c.status == Some("Active".to_string()),
                deposit: c.deposit_enabled,
                withdraw: c.withdraw_enabled,
                fee: None,
                precision: c.scale.map(|s| s as i32),
                limits: None,
                networks: HashMap::new(),
                info: serde_json::to_value(&c).unwrap_or_default(),
            };
            currencies.insert(c.currency.clone(), currency);
        }

        Ok(currencies)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        if let Some(n) = network {
            params.insert("chainName".into(), n.to_string());
        }

        let response: PhemexDepositAddressResponse = self
            .private_request(
                "GET",
                "/phemex-user/wallets/v2/depositAddress",
                Some(params),
            )
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "DepositAddress".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(crate::types::DepositAddress {
            info: serde_json::to_value(&data).unwrap_or_default(),
            currency: code.to_string(),
            network: data.chain_name,
            address: data.address.unwrap_or_default(),
            tag: data.tag,
        })
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: PhemexTransactionsResponse = self
            .private_request(
                "GET",
                "/phemex-user/wallets/deposits",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let transactions: Vec<crate::types::Transaction> = data
            .iter()
            .map(|t| self.parse_transaction(t, "deposit"))
            .collect();

        Ok(transactions)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(s) = since {
            params.insert("start".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: PhemexTransactionsResponse = self
            .private_request(
                "GET",
                "/phemex-user/wallets/withdrawals",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let transactions: Vec<crate::types::Transaction> = data
            .iter()
            .map(|t| self.parse_transaction(t, "withdrawal"))
            .collect();

        Ok(transactions)
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::Transaction> {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let amount_scaled = (amount * scale).to_string();

        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        params.insert("amountEv".into(), amount_scaled);
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("addressTag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("chainName".into(), n.to_string());
        }

        let response: PhemexWithdrawResponse = self
            .private_request("POST", "/phemex-user/wallets/withdrawals", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Withdrawal".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(crate::types::Transaction {
            info: serde_json::to_value(&data).unwrap_or_default(),
            id: data.id.unwrap_or_default(),
            txid: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            address: Some(address.to_string()),
            tag: tag.map(|s| s.to_string()),
            tx_type: crate::types::TransactionType::Withdrawal,
            amount,
            currency: code.to_string(),
            status: crate::types::TransactionStatus::Pending,
            updated: None,
            fee: None,
            network: network.map(|s| s.to_string()),
            internal: None,
            confirmations: None,
        })
    }
}

impl Phemex {
    fn parse_transaction(
        &self,
        data: &PhemexTransaction,
        transaction_type: &str,
    ) -> crate::types::Transaction {
        let scale = Decimal::new(10_i64.pow(8), 0);
        let amount = data
            .amount_ev
            .map(|a| Decimal::from(a) / scale)
            .unwrap_or_default();

        let status = match data.status.as_deref() {
            Some("Success") | Some("Completed") => crate::types::TransactionStatus::Ok,
            Some("Pending") => crate::types::TransactionStatus::Pending,
            Some("Failed") | Some("Rejected") => crate::types::TransactionStatus::Failed,
            Some("Cancelled") => crate::types::TransactionStatus::Canceled,
            _ => crate::types::TransactionStatus::Pending,
        };

        crate::types::Transaction {
            info: serde_json::to_value(data).unwrap_or_default(),
            id: data.id.clone().unwrap_or_default(),
            txid: data.txid.clone(),
            timestamp: data.create_time,
            datetime: data
                .create_time
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            address: data.address.clone(),
            tag: data.address_tag.clone(),
            tx_type: if transaction_type == "deposit" {
                crate::types::TransactionType::Deposit
            } else {
                crate::types::TransactionType::Withdrawal
            },
            amount,
            currency: data.currency.clone().unwrap_or_default(),
            status,
            updated: data.update_time,
            fee: None,
            network: data.chain_name.clone(),
            internal: None,
            confirmations: None,
        }
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct PhemexResponse<T> {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexMdResponse<T> {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    result: Option<T>,
}

type PhemexProductsResponse = PhemexResponse<PhemexProductsData>;
type PhemexTickerResponse = PhemexMdResponse<PhemexTicker>;
type PhemexTickersResponse = PhemexMdResponse<Vec<PhemexTicker>>;
type PhemexOrderBookResponse = PhemexMdResponse<PhemexOrderBookData>;
type PhemexTradesResponse = PhemexMdResponse<PhemexTradesData>;
type PhemexKlinesResponse = PhemexMdResponse<PhemexKlinesData>;
type PhemexBalanceResponse = PhemexResponse<Vec<PhemexWallet>>;
type PhemexOrderResponse = PhemexResponse<PhemexOrder>;
type PhemexOrdersResponse = PhemexResponse<PhemexOrdersData>;
type PhemexMyTradesResponse = PhemexResponse<PhemexTradesListData>;

#[derive(Debug, Default, Deserialize)]
struct PhemexProductsData {
    #[serde(default)]
    products: Vec<PhemexProduct>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexProduct {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    base_currency: Option<String>,
    #[serde(default)]
    quote_currency: Option<String>,
    #[serde(default)]
    settle_currency: Option<String>,
    #[serde(default, rename = "type")]
    product_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tick_size: Option<i64>,
    #[serde(default)]
    lot_size: Option<i64>,
    #[serde(default)]
    contract_size: Option<i64>,
    #[serde(default)]
    taker_fee_rate_er: Option<i64>,
    #[serde(default)]
    maker_fee_rate_er: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    last_ep: Option<i64>,
    #[serde(default)]
    bid_ep: Option<i64>,
    #[serde(default)]
    ask_ep: Option<i64>,
    #[serde(default)]
    open_ep: Option<i64>,
    #[serde(default)]
    high_ep: Option<i64>,
    #[serde(default)]
    low_ep: Option<i64>,
    #[serde(default)]
    volume_ev: Option<i64>,
    #[serde(default)]
    turnover_ev: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexOrderBookData {
    #[serde(default)]
    book: PhemexBook,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexBook {
    #[serde(default)]
    bids: Vec<Vec<i64>>,
    #[serde(default)]
    asks: Vec<Vec<i64>>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexTradesData {
    #[serde(default)]
    trades: Vec<PhemexTrade>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexTrade {
    #[serde(default, rename = "tradeId")]
    trade_id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price_ep: Option<i64>,
    #[serde(default)]
    qty: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexKlinesData {
    #[serde(default)]
    rows: Vec<Vec<i64>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhemexWallet {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    balance_ev: Option<i64>,
    #[serde(default)]
    available_balance_ev: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexOrder {
    #[serde(default, rename = "orderID")]
    order_id: Option<String>,
    #[serde(default, rename = "clOrdID")]
    cl_ord_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    ord_type: Option<String>,
    #[serde(default)]
    ord_status: Option<String>,
    #[serde(default)]
    price_ep: Option<i64>,
    #[serde(default)]
    order_qty: Option<i64>,
    #[serde(default)]
    cum_qty: Option<i64>,
    #[serde(default)]
    cum_value_ev: Option<i64>,
    #[serde(default)]
    stop_px_ep: Option<i64>,
    #[serde(default)]
    create_time_ns: Option<i64>,
    #[serde(default)]
    action_time_ns: Option<i64>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    post_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexOrdersData {
    #[serde(default)]
    rows: Vec<PhemexOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct PhemexTradesListData {
    #[serde(default)]
    rows: Vec<PhemexTrade>,
}

// Currency types
type PhemexCurrenciesResponse = PhemexResponse<Vec<PhemexCurrency>>;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexCurrency {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    scale: Option<i64>,
    #[serde(default)]
    deposit_enabled: Option<bool>,
    #[serde(default)]
    withdraw_enabled: Option<bool>,
}

// Deposit address types
type PhemexDepositAddressResponse = PhemexResponse<PhemexDepositAddressData>;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexDepositAddressData {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    chain_name: Option<String>,
}

// Transaction types
type PhemexTransactionsResponse = PhemexResponse<Vec<PhemexTransaction>>;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexTransaction {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    amount_ev: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_tag: Option<String>,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    chain_name: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
}

// Withdraw types
type PhemexWithdrawResponse = PhemexResponse<PhemexWithdrawData>;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhemexWithdrawData {
    #[serde(default)]
    id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Phemex::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Phemex);
        assert_eq!(exchange.name(), "Phemex");
        assert!(exchange.has().spot);
        assert!(exchange.has().swap);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Phemex::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USDT"), "BTCUSDT");
    }
}
