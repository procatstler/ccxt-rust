//! BitMart Exchange Implementation
//!
//! BitMart 거래소 구현체

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

/// BitMart 거래소
pub struct Bitmart {
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

impl Bitmart {
    const BASE_URL: &'static str = "https://api-cloud.bitmart.com";
    const RATE_LIMIT_MS: u64 = 34; // 30 requests per second

    /// 새 Bitmart 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
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
            fetch_orders: false,
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
        api_urls.insert("spot".into(), "https://api-cloud.bitmart.com".into());
        api_urls.insert("swap".into(), "https://api-cloud-v2.bitmart.com".into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/0623e9c4-f50e-48c9-82bd-65c3908c3a14"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://www.bitmart.com".into()),
            doc: vec!["https://developer-pro.bitmart.com/".into()],
            fees: Some("https://www.bitmart.com/fee/en".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute3, "3".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour2, "120".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Hour6, "360".into());
        timeframes.insert(Timeframe::Hour12, "720".into());
        timeframes.insert(Timeframe::Day1, "1440".into());
        timeframes.insert(Timeframe::Week1, "10080".into());

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
        let memo = self.config.uid().unwrap_or("");

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Build body for POST requests
        let body = if method == "POST" {
            params
                .as_ref()
                .map(|p| serde_json::to_string(p).unwrap_or_default())
        } else {
            None
        };

        // Sign: timestamp + "#" + memo + "#" + body
        let sign_str = if let Some(ref b) = body {
            format!("{timestamp}#{memo}#{b}")
        } else {
            format!("{timestamp}#{memo}")
        };

        let signature = if !api_secret.is_empty() {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(sign_str.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        } else {
            String::new()
        };

        let mut headers = HashMap::new();
        headers.insert("X-BM-KEY".into(), api_key.to_string());
        headers.insert("X-BM-SIGN".into(), signature);
        headers.insert("X-BM-TIMESTAMP".into(), timestamp);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => {
                let url = if let Some(ref p) = params {
                    let query: String = p
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                        .collect::<Vec<_>>()
                        .join("&");
                    format!("{path}?{query}")
                } else {
                    path.to_string()
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
                self.private_client
                    .delete(path, params, Some(headers))
                    .await
            },
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 파싱
    fn parse_market(&self, data: &BitmartMarket) -> Market {
        let symbol = self.convert_to_symbol(&data.symbol);
        let parts: Vec<&str> = symbol.split('/').collect();
        let (base, quote) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (symbol.clone(), "USDT".to_string())
        };

        Market {
            id: data.symbol.clone(),
            lowercase_id: Some(data.symbol.to_lowercase()),
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
            active: true,
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
            settle: None,
            settle_id: None,
            precision: MarketPrecision {
                amount: data.base_min_size.as_ref().map(|_| 8),
                price: data.price_max_precision,
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
    fn parse_ticker(&self, data: &BitmartTicker) -> Ticker {
        let symbol = self.convert_to_symbol(&data.symbol);
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_24h.as_ref().and_then(|s| s.parse().ok()),
            low: data.low_24h.as_ref().and_then(|s| s.parse().ok()),
            bid: data.best_bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: data.best_bid_size.as_ref().and_then(|s| s.parse().ok()),
            ask: data.best_ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: data.best_ask_size.as_ref().and_then(|s| s.parse().ok()),
            vwap: None,
            open: data.open_24h.as_ref().and_then(|s| s.parse().ok()),
            close: data.last.as_ref().and_then(|s| s.parse().ok()),
            last: data.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.fluctuation.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.base_volume_24h.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: data.quote_volume_24h.as_ref().and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, data: &BitmartOrder) -> Order {
        let symbol = self.convert_to_symbol(&data.symbol);
        let timestamp = data
            .create_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.state.as_deref() {
            Some("new") | Some("partially_filled") => OrderStatus::Open,
            Some("filled") => OrderStatus::Closed,
            Some("canceled") | Some("partially_canceled") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
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

        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .size
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .filled_size
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let remaining = amount - filled;
        let cost: Decimal = data
            .filled_notional
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: if filled > Decimal::ZERO {
                Some(cost / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(remaining),
            cost: Some(cost),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        }
    }

    /// 거래 파싱
    fn parse_trade(&self, data: &BitmartTrade, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.amount.parse().unwrap_or_default();

        let side = if data.side.as_deref() == Some("buy") {
            "buy"
        } else {
            "sell"
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
    fn parse_ohlcv(&self, data: &[String]) -> Option<OHLCV> {
        if data.len() < 6 {
            return None;
        }
        let timestamp = data[0].parse::<i64>().ok()? * 1000;
        Some(OHLCV {
            timestamp,
            open: data[1].parse().ok()?,
            high: data[2].parse().ok()?,
            low: data[3].parse().ok()?,
            close: data[4].parse().ok()?,
            volume: data[5].parse().ok()?,
        })
    }

    /// 심볼을 Bitmart 형식으로 변환 (BTC/USDT -> BTC_USDT)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// Bitmart 심볼을 통합 형식으로 변환 (BTC_USDT -> BTC/USDT)
    fn convert_to_symbol(&self, market_id: &str) -> String {
        market_id.replace("_", "/")
    }
}

#[async_trait]
impl Exchange for Bitmart {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitmart
    }

    fn name(&self) -> &str {
        "BitMart"
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

        let timestamp = Utc::now().timestamp_millis().to_string();
        let api_key = self.config.api_key().unwrap_or("");
        let api_secret = self.config.secret().unwrap_or("");
        let memo = self.config.uid().unwrap_or("");

        // Body for POST requests
        let body = if method == "POST" && !params.is_empty() {
            Some(serde_json::to_string(params).unwrap_or_default())
        } else {
            None
        };

        // Sign: timestamp + "#" + memo + "#" + body
        let sign_str = if let Some(ref b) = body {
            format!("{timestamp}#{memo}#{b}")
        } else {
            format!("{timestamp}#{memo}")
        };

        let signature = if !api_secret.is_empty() {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(sign_str.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        } else {
            String::new()
        };

        let mut headers = HashMap::new();
        headers.insert("X-BM-KEY".into(), api_key.to_string());
        headers.insert("X-BM-SIGN".into(), signature);
        headers.insert("X-BM-TIMESTAMP".into(), timestamp);
        headers.insert("Content-Type".into(), "application/json".into());

        // For GET requests, append query string to URL
        let url = if method == "GET" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
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
        let response: BitmartMarketsResponse =
            self.public_get("/spot/v1/symbols/details", None).await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Markets".to_string(),
            message: "No data in response".into(),
        })?;

        let markets: Vec<Market> = data.symbols.iter().map(|m| self.parse_market(m)).collect();

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

        let response: BitmartTickerResponse = self
            .public_get("/spot/quotation/v3/ticker", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Ticker".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(self.parse_ticker(&data))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BitmartTickersResponse =
            self.public_get("/spot/quotation/v3/tickers", None).await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Tickers".to_string(),
            message: "No data in response".into(),
        })?;

        let mut result = HashMap::new();
        for t in &data {
            let ticker = self.parse_ticker(t);
            if let Some(filter) = symbols {
                if filter.contains(&ticker.symbol.as_str()) {
                    result.insert(ticker.symbol.clone(), ticker);
                }
            } else {
                result.insert(ticker.symbol.clone(), ticker);
            }
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BitmartOrderBookResponse = self
            .public_get("/spot/quotation/v3/books", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data in response".into(),
        })?;

        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    let price = b[0].parse::<Decimal>().ok()?;
                    let amount = b[1].parse::<Decimal>().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    let price = a[0].parse::<Decimal>().ok()?;
                    let amount = a[1].parse::<Decimal>().ok()?;
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
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(50).to_string());
        }

        let response: BitmartTradesResponse = self
            .public_get("/spot/quotation/v3/trades", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Trades".to_string(),
            message: "No data in response".into(),
        })?;

        let trades: Vec<Trade> = data.iter().map(|t| self.parse_trade(t, symbol)).collect();

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
        let tf = self
            .timeframes
            .get(&timeframe)
            .cloned()
            .unwrap_or("60".into());

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("step".into(), tf);

        if let Some(s) = since {
            params.insert("before".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(500).to_string());
        }

        let response: BitmartOhlcvResponse = self
            .public_get("/spot/quotation/v3/klines", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "OHLCV".to_string(),
            message: "No data in response".into(),
        })?;

        let ohlcv: Vec<OHLCV> = data.iter().filter_map(|k| self.parse_ohlcv(k)).collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: BitmartBalanceResponse =
            self.private_request("GET", "/spot/v1/wallet", None).await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
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

        for b in data.wallet {
            let free: Decimal = b.available.parse().unwrap_or_default();
            let used: Decimal = b.frozen.parse().unwrap_or_default();
            let total = free + used;

            balances.currencies.insert(
                b.id.clone(),
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

        let mut request = HashMap::new();
        request.insert("symbol".into(), market_id);
        request.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "buy".into(),
                OrderSide::Sell => "sell".into(),
            },
        );
        request.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "limit".into(),
                OrderType::Market => "market".into(),
                _ => "limit".into(),
            },
        );
        request.insert("size".into(), amount.to_string());

        if let Some(p) = price {
            request.insert("price".into(), p.to_string());
        }

        let response: BitmartOrderResponse = self
            .private_request("POST", "/spot/v2/submit_order", Some(request))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Order".to_string(),
            message: "No data in response".into(),
        })?;

        let order_id = data.order_id.clone().unwrap_or_default();
        Ok(Order {
            id: order_id,
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            cost: None,
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

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("order_id".into(), id.to_string());

        let response: BitmartCancelResponse = self
            .private_request("POST", "/spot/v3/cancel_order", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
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
        params.insert("order_id".into(), id.to_string());

        let response: BitmartOrderDetailResponse = self
            .private_request("GET", "/spot/v2/order_detail", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
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
        params.insert("status".into(), "1".into()); // 1 = open

        let response: BitmartOrdersResponse = self
            .private_request("GET", "/spot/v3/orders", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.orders.iter().map(|o| self.parse_order(o)).collect();

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
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }

        let response: BitmartMyTradesResponse = self
            .private_request("GET", "/spot/v2/trades", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Trades".to_string(),
            message: "No data in response".into(),
        })?;

        let trades: Vec<Trade> = data
            .trades
            .iter()
            .map(|t| {
                let sym = symbol
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.convert_to_symbol(&t.symbol));
                self.parse_trade(
                    &BitmartTrade {
                        trade_id: t.detail_id.clone(),
                        price: t.price.clone().unwrap_or_default(),
                        amount: t.size.clone().unwrap_or_default(),
                        side: t.side.clone(),
                        timestamp: t.create_time,
                    },
                    &sym,
                )
            })
            .collect();

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

        let _response: BitmartResponse<serde_json::Value> = self
            .private_request(
                "POST",
                "/spot/v1/cancel_all_orders",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        Ok(vec![])
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".into(), self.convert_to_market_id(s));
        }
        params.insert("status".into(), "6".into()); // 6 = filled

        let response: BitmartOrdersResponse = self
            .private_request("GET", "/spot/v3/orders", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.orders.iter().map(|o| self.parse_order(o)).collect();
        Ok(orders)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        let response: BitmartCurrenciesResponse =
            self.public_get("/spot/v1/currencies", None).await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Currencies".to_string(),
            message: "No data in response".into(),
        })?;

        let mut currencies = HashMap::new();
        for c in data.currencies {
            let currency = crate::types::Currency {
                id: c.id.clone(),
                code: c.id.clone(),
                name: c.name.clone(),
                active: true,
                deposit: Some(c.deposit_enabled.unwrap_or(false)),
                withdraw: Some(c.withdraw_enabled.unwrap_or(false)),
                fee: None,
                precision: None,
                limits: None,
                networks: HashMap::new(),
                info: serde_json::to_value(&c).unwrap_or_default(),
            };
            currencies.insert(c.id, currency);
        }

        Ok(currencies)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());

        let response: BitmartDepositAddressResponse = self
            .private_request("GET", "/account/v1/deposit/address", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "DepositAddress".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(crate::types::DepositAddress {
            info: serde_json::to_value(&data).unwrap_or_default(),
            currency: code.to_string(),
            network: data.chain,
            address: data.address.unwrap_or_default(),
            tag: data.address_memo,
        })
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        params.insert("operation_type".into(), "deposit".into());

        let response: BitmartTransactionsResponse = self
            .private_request(
                "GET",
                "/account/v2/deposit-withdraw/history",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Transactions".to_string(),
            message: "No data in response".into(),
        })?;

        let transactions: Vec<crate::types::Transaction> = data
            .records
            .iter()
            .map(|t| self.parse_transaction(t, "deposit"))
            .collect();

        Ok(transactions)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }
        params.insert("operation_type".into(), "withdraw".into());

        let response: BitmartTransactionsResponse = self
            .private_request(
                "GET",
                "/account/v2/deposit-withdraw/history",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Transactions".to_string(),
            message: "No data in response".into(),
        })?;

        let transactions: Vec<crate::types::Transaction> = data
            .records
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
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("address_memo".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("destination".into(), n.to_string());
        }

        let response: BitmartWithdrawResponse = self
            .private_request("POST", "/account/v1/withdraw/apply", Some(params))
            .await?;

        if response.code != 1000 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Withdrawal".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(crate::types::Transaction {
            info: serde_json::to_value(&data).unwrap_or_default(),
            id: data.withdraw_id.unwrap_or_default(),
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

impl Bitmart {
    fn parse_transaction(
        &self,
        data: &BitmartTransaction,
        transaction_type: &str,
    ) -> crate::types::Transaction {
        let amount = data
            .amount
            .as_ref()
            .and_then(|a| a.parse::<Decimal>().ok())
            .unwrap_or_default();

        let status = match data.status.as_deref() {
            Some("0") | Some("3") => crate::types::TransactionStatus::Ok,
            Some("1") | Some("2") => crate::types::TransactionStatus::Pending,
            Some("4") | Some("5") => crate::types::TransactionStatus::Failed,
            _ => crate::types::TransactionStatus::Pending,
        };

        crate::types::Transaction {
            info: serde_json::to_value(data).unwrap_or_default(),
            id: data.withdraw_id.clone().unwrap_or_default(),
            txid: data.tx_id.clone(),
            timestamp: data.apply_time,
            datetime: data
                .apply_time
                .and_then(|t| chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())),
            address: data.address.clone(),
            tag: data.address_memo.clone(),
            tx_type: if transaction_type == "deposit" {
                crate::types::TransactionType::Deposit
            } else {
                crate::types::TransactionType::Withdrawal
            },
            amount,
            currency: data.currency.clone().unwrap_or_default(),
            status,
            updated: None,
            fee: None,
            network: data.network.clone(),
            internal: None,
            confirmations: None,
        }
    }
}

// === Response Types ===

#[derive(Debug, Deserialize)]
struct BitmartResponse<T> {
    code: i32,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

type BitmartMarketsResponse = BitmartResponse<BitmartMarketsData>;
type BitmartTickerResponse = BitmartResponse<BitmartTicker>;
type BitmartTickersResponse = BitmartResponse<Vec<BitmartTicker>>;
type BitmartOrderBookResponse = BitmartResponse<BitmartOrderBookData>;
type BitmartTradesResponse = BitmartResponse<Vec<BitmartTrade>>;
type BitmartOhlcvResponse = BitmartResponse<Vec<Vec<String>>>;
type BitmartBalanceResponse = BitmartResponse<BitmartBalanceData>;
type BitmartOrderResponse = BitmartResponse<BitmartOrderResult>;
type BitmartCancelResponse = BitmartResponse<serde_json::Value>;
type BitmartOrderDetailResponse = BitmartResponse<BitmartOrder>;
type BitmartOrdersResponse = BitmartResponse<BitmartOrdersData>;
type BitmartMyTradesResponse = BitmartResponse<BitmartMyTradesData>;
type BitmartCurrenciesResponse = BitmartResponse<BitmartCurrenciesData>;
type BitmartDepositAddressResponse = BitmartResponse<BitmartDepositAddressData>;
type BitmartTransactionsResponse = BitmartResponse<BitmartTransactionsData>;
type BitmartWithdrawResponse = BitmartResponse<BitmartWithdrawData>;

#[derive(Debug, Default, Deserialize)]
struct BitmartMarketsData {
    #[serde(default)]
    symbols: Vec<BitmartMarket>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmartMarket {
    symbol: String,
    #[serde(default)]
    base_currency: Option<String>,
    #[serde(default)]
    quote_currency: Option<String>,
    #[serde(default)]
    base_min_size: Option<String>,
    #[serde(default)]
    price_max_precision: Option<i32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartTicker {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open_24h: Option<String>,
    #[serde(default)]
    high_24h: Option<String>,
    #[serde(default)]
    low_24h: Option<String>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
    #[serde(default)]
    base_volume_24h: Option<String>,
    #[serde(default)]
    quote_volume_24h: Option<String>,
    #[serde(default)]
    fluctuation: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitmartTrade {
    #[serde(default)]
    trade_id: Option<String>,
    #[serde(default)]
    price: String,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartBalanceData {
    #[serde(default)]
    wallet: Vec<BitmartWallet>,
}

#[derive(Debug, Deserialize)]
struct BitmartWallet {
    id: String,
    #[serde(default)]
    available: String,
    #[serde(default)]
    frozen: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    filled_size: Option<String>,
    #[serde(default)]
    filled_notional: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartOrderResult {
    #[serde(default)]
    order_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartOrdersData {
    #[serde(default)]
    orders: Vec<BitmartOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartMyTradesData {
    #[serde(default)]
    trades: Vec<BitmartMyTrade>,
}

#[derive(Debug, Deserialize)]
struct BitmartMyTrade {
    #[serde(default)]
    detail_id: Option<String>,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartCurrenciesData {
    #[serde(default)]
    currencies: Vec<BitmartCurrency>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartCurrency {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    deposit_enabled: Option<bool>,
    #[serde(default)]
    withdraw_enabled: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartDepositAddressData {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_memo: Option<String>,
    #[serde(default)]
    chain: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BitmartTransactionsData {
    #[serde(default)]
    records: Vec<BitmartTransaction>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartTransaction {
    #[serde(default)]
    withdraw_id: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_memo: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    apply_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BitmartWithdrawData {
    #[serde(default)]
    withdraw_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitmart::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitmart);
        assert_eq!(exchange.name(), "BitMart");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bitmart::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USDT"), "BTC_USDT");
        assert_eq!(exchange.convert_to_symbol("BTC_USDT"), "BTC/USDT");
    }
}
