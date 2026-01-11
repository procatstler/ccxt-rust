//! BitoPro Exchange Implementation
//!
//! Taiwan cryptocurrency exchange (幣託)

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha384;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha384 = Hmac<Sha384>;

/// BitoPro exchange
pub struct Bitopro {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// API response structures
#[derive(Debug, Deserialize)]
struct BitoproTickerResponse {
    data: BitoproTickerData,
}

#[derive(Debug, Deserialize)]
struct BitoproTickerData {
    pair: String,
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "isBuyer")]
    is_buyer: Option<bool>,
    #[serde(rename = "priceChange24hr")]
    price_change_24hr: String,
    volume24hr: String,
    high24hr: String,
    low24hr: String,
}

#[derive(Debug, Deserialize)]
struct BitoproTickersResponse {
    data: Vec<BitoproTickerItem>,
}

#[derive(Debug, Deserialize)]
struct BitoproTickerItem {
    pair: String,
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "priceChange24hr")]
    price_change_24hr: String,
    volume24hr: String,
    high24hr: String,
    low24hr: String,
}

#[derive(Debug, Deserialize)]
struct BitoproOrderBookResponse {
    bids: Vec<BitoproOrderBookEntry>,
    asks: Vec<BitoproOrderBookEntry>,
}

#[derive(Debug, Deserialize)]
struct BitoproOrderBookEntry {
    price: String,
    amount: String,
    count: i32,
    total: String,
}

#[derive(Debug, Deserialize)]
struct BitoproTradesResponse {
    data: Vec<BitoproTrade>,
}

#[derive(Debug, Deserialize)]
struct BitoproTrade {
    #[serde(rename = "timestamp")]
    timestamp: i64,
    price: String,
    amount: String,
    #[serde(rename = "isBuyer")]
    is_buyer: bool,
}

#[derive(Debug, Deserialize)]
struct BitoproTradingPairsResponse {
    data: Vec<BitoproTradingPair>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitoproTradingPair {
    pair: String,
    base: String,
    quote: String,
    #[serde(rename = "basePrecision")]
    base_precision: i32,
    #[serde(rename = "quotePrecision")]
    quote_precision: i32,
    #[serde(rename = "minLimitBaseAmount")]
    min_limit_base_amount: String,
    #[serde(rename = "maxLimitBaseAmount")]
    max_limit_base_amount: String,
    #[serde(rename = "minMarketBuyQuoteAmount")]
    min_market_buy_quote_amount: Option<String>,
    #[serde(rename = "orderOpenLimit")]
    order_open_limit: Option<i32>,
    maintain: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct BitoproBalanceResponse {
    data: Vec<BitoproBalance>,
}

#[derive(Debug, Deserialize)]
struct BitoproBalance {
    currency: String,
    amount: String,
    available: String,
    stake: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitoproOrderResponse {
    #[serde(rename = "orderId")]
    order_id: String,
    action: String,
    price: String,
    #[serde(rename = "avgExecutionPrice")]
    avg_execution_price: Option<String>,
    #[serde(rename = "type")]
    order_type: String,
    timestamp: i64,
    status: i32,
    #[serde(rename = "originalAmount")]
    original_amount: String,
    #[serde(rename = "remainingAmount")]
    remaining_amount: String,
    #[serde(rename = "executedAmount")]
    executed_amount: Option<String>,
    pair: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitoproOrdersResponse {
    data: Vec<BitoproOrderResponse>,
}

#[derive(Debug, Deserialize)]
struct BitoproCreateOrderResponse {
    #[serde(rename = "orderId")]
    order_id: String,
    action: String,
    amount: String,
    price: String,
    timestamp: i64,
}

#[derive(Debug, Deserialize)]
struct BitoproOHLCVResponse {
    data: Vec<Vec<serde_json::Value>>,
}

impl Bitopro {
    const BASE_URL: &'static str = "https://api.bitopro.com/v3";
    const RATE_LIMIT_MS: u64 = 100; // 600 requests/min = 10/sec

    /// Create new BitoPro instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
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
            create_market_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87591171-9a377d80-c6f0-11ea-94ac-97a126eac3bc.jpg".into()),
            api: api_urls,
            www: Some("https://www.bitopro.com".into()),
            doc: vec!["https://github.com/bitoex/bitopro-offical-api-docs".into()],
            fees: Some("https://www.bitopro.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour6, "6h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());
        timeframes.insert(Timeframe::Month1, "1M".to_string());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
        })
    }

    /// Public API request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// Private API request with authentication
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = Utc::now().timestamp_millis();

        // Build payload
        let (payload_json, post_body) = if method == "GET" || method == "DELETE" {
            // For GET/DELETE, use identity-based payload
            let email = self.config.uid().unwrap_or("user@bitopro.com");
            (serde_json::json!({
                "identity": email,
                "nonce": nonce
            }), None)
        } else {
            // For POST/PUT, use body with nonce
            let mut payload = body.clone().unwrap_or(serde_json::json!({}));
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("nonce".to_string(), serde_json::json!(nonce));
            }
            (payload.clone(), Some(payload))
        };

        let payload_str = serde_json::to_string(&payload_json)
            .map_err(|e| CcxtError::ExchangeError { message: e.to_string() })?;
        let payload_base64 = BASE64.encode(payload_str.as_bytes());

        // Generate signature
        let mut mac = HmacSha384::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(payload_base64.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("X-BITOPRO-APIKEY".into(), api_key.to_string());
        headers.insert("X-BITOPRO-PAYLOAD".into(), payload_base64);
        headers.insert("X-BITOPRO-SIGNATURE".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => self.client.post(path, post_body, Some(headers)).await,
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::ExchangeError { message: format!("Unsupported method: {method}") }),
        }
    }

    /// Convert symbol to market ID (BTC/TWD -> btc_twd)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace('/', "_").to_lowercase()
    }

    /// Convert market ID to symbol (btc_twd -> BTC/TWD)
    fn convert_to_symbol(&self, market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            market_id.to_uppercase()
        }
    }

    /// Get market ID from symbol
    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            Ok(market.id.clone())
        } else {
            Ok(self.convert_to_market_id(symbol))
        }
    }

    /// Parse order status from API code
    fn parse_order_status(status: i32) -> OrderStatus {
        match status {
            -1 => OrderStatus::Canceled,
            0 => OrderStatus::Open,
            1 => OrderStatus::Open,        // Partially filled
            2 => OrderStatus::Closed,      // Fully filled
            3 => OrderStatus::Canceled,    // Partially filled then canceled
            _ => OrderStatus::Open,        // Default to Open for unknown status
        }
    }

    /// Parse order side
    fn parse_order_side(action: &str) -> OrderSide {
        match action.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order type
    fn parse_order_type(order_type: &str) -> OrderType {
        match order_type.to_lowercase().as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            "stop_limit" => OrderType::StopLimit,
            _ => OrderType::Limit,
        }
    }
}

#[async_trait]
impl Exchange for Bitopro {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitopro
    }

    fn name(&self) -> &str {
        "BitoPro"
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

        let response: BitoproTradingPairsResponse = self.public_get("/provisioning/trading-pairs", None).await?;
        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for pair in response.data {
            if pair.maintain.unwrap_or(false) {
                continue; // Skip pairs under maintenance
            }

            let base = pair.base.to_uppercase();
            let quote = pair.quote.to_uppercase();
            let symbol = format!("{base}/{quote}");
            let market_id = pair.pair.clone();

            let min_amount = Decimal::from_str(&pair.min_limit_base_amount).ok();
            let max_amount = Decimal::from_str(&pair.max_limit_base_amount).ok();

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: pair.base.clone(),
                quote_id: pair.quote.clone(),
                active: true,
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                contract: false,
                settle: None,
                settle_id: None,
                contract_size: None,
                linear: None,
                inverse: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                taker: Some(Decimal::from_str("0.002").unwrap()),
                maker: Some(Decimal::from_str("0.001").unwrap()),
                precision: MarketPrecision {
                    amount: Some(pair.base_precision),
                    price: Some(pair.quote_precision),
                    cost: Some(pair.quote_precision),
                    base: Some(pair.base_precision),
                    quote: Some(pair.quote_precision),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax { min: min_amount, max: max_amount },
                    price: crate::types::MinMax { min: None, max: None },
                    cost: crate::types::MinMax { min: None, max: None },
                    leverage: crate::types::MinMax { min: None, max: None },
                },
                info: serde_json::to_value(&pair).unwrap_or_default(),
                index: false,
                sub_type: None,
                margin_modes: None,
                tier_based: false,
                percentage: true,
                created: None,
            };

            markets_by_id.insert(market_id, symbol.clone());
            markets.insert(symbol, market);
        }

        {
            let mut m = self.markets.write().unwrap();
            *m = markets.clone();
        }
        {
            let mut m = self.markets_by_id.write().unwrap();
            *m = markets_by_id;
        }

        Ok(markets)
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
        _api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let url = format!("{}{}", Self::BASE_URL, path);

        let final_url = if !params.is_empty() && method == "GET" {
            let query: String = params.iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{url}?{query}")
        } else {
            url
        };

        SignedRequest {
            url: final_url,
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/tickers/{market_id}");
        let response: BitoproTickerResponse = self.public_get(&path, None).await?;
        let data = response.data;

        let last = Decimal::from_str(&data.last_price).ok();
        let high = Decimal::from_str(&data.high24hr).ok();
        let low = Decimal::from_str(&data.low24hr).ok();
        let change = Decimal::from_str(&data.price_change_24hr).ok();
        let volume = Decimal::from_str(&data.volume24hr).ok();

        let percentage = if let (Some(c), Some(l)) = (change, last) {
            if l != Decimal::ZERO {
                let prev = l - c;
                if prev != Decimal::ZERO {
                    Some((c / prev) * Decimal::from(100))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            high,
            low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume: volume,
            quote_volume: None,
            info: serde_json::json!({}),
            index_price: None,
            mark_price: None,
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BitoproTickersResponse = self.public_get("/tickers", None).await?;
        let mut tickers = HashMap::new();

        for item in response.data {
            let symbol = self.convert_to_symbol(&item.pair);

            if let Some(syms) = symbols {
                if !syms.contains(&symbol.as_str()) {
                    continue;
                }
            }

            let last = Decimal::from_str(&item.last_price).ok();
            let high = Decimal::from_str(&item.high24hr).ok();
            let low = Decimal::from_str(&item.low24hr).ok();
            let change = Decimal::from_str(&item.price_change_24hr).ok();
            let volume = Decimal::from_str(&item.volume24hr).ok();

            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: Some(Utc::now().timestamp_millis()),
                datetime: Some(Utc::now().to_rfc3339()),
                high,
                low,
                bid: None,
                bid_volume: None,
                ask: None,
                ask_volume: None,
                vwap: None,
                open: None,
                close: last,
                last,
                previous_close: None,
                change,
                percentage: None,
                average: None,
                base_volume: volume,
                quote_volume: None,
                info: serde_json::json!({}),
                index_price: None,
                mark_price: None,
            };

            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let path = format!("/order-book/{market_id}");
        let response: BitoproOrderBookResponse = self.public_get(&path, Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response.bids
            .iter()
            .map(|b| OrderBookEntry {
                price: Decimal::from_str(&b.price).unwrap_or(Decimal::ZERO),
                amount: Decimal::from_str(&b.amount).unwrap_or(Decimal::ZERO),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response.asks
            .iter()
            .map(|a| OrderBookEntry {
                price: Decimal::from_str(&a.price).unwrap_or(Decimal::ZERO),
                amount: Decimal::from_str(&a.amount).unwrap_or(Decimal::ZERO),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/trades/{market_id}");
        let response: BitoproTradesResponse = self.public_get(&path, None).await?;

        let mut trades: Vec<Trade> = response.data
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let price = Decimal::from_str(&t.price).unwrap_or(Decimal::ZERO);
                let amount = Decimal::from_str(&t.amount).unwrap_or(Decimal::ZERO);

                Trade {
                    id: format!("{}_{}", t.timestamp, i),
                    info: serde_json::json!({}),
                    timestamp: Some(t.timestamp),
                    datetime: Some(chrono::DateTime::from_timestamp_millis(t.timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()),
                    symbol: symbol.to_string(),
                    order: None,
                    trade_type: None,
                    side: Some(if t.is_buyer { "buy".to_string() } else { "sell".to_string() }),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: vec![],
                }
            })
            .collect();

        if let Some(l) = limit {
            trades.truncate(l as usize);
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
        let market_id = self.get_market_id(symbol)?;
        let tf = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest { message: "Unsupported timeframe".into() })?;

        let mut params = HashMap::new();
        params.insert("resolution".to_string(), tf.clone());
        if let Some(s) = since {
            params.insert("from".to_string(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("to".to_string(), (Utc::now().timestamp() + (l as i64 * 86400)).to_string());
        }

        let path = format!("/trading-history/{market_id}");
        let response: BitoproOHLCVResponse = self.public_get(&path, Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response.data
            .iter()
            .filter_map(|candle| {
                if candle.len() >= 6 {
                    Some(OHLCV {
                        timestamp: candle[0].as_i64().unwrap_or(0) * 1000,
                        open: Decimal::from_str(candle[1].as_str().unwrap_or("0")).unwrap_or(Decimal::ZERO),
                        high: Decimal::from_str(candle[2].as_str().unwrap_or("0")).unwrap_or(Decimal::ZERO),
                        low: Decimal::from_str(candle[3].as_str().unwrap_or("0")).unwrap_or(Decimal::ZERO),
                        close: Decimal::from_str(candle[4].as_str().unwrap_or("0")).unwrap_or(Decimal::ZERO),
                        volume: Decimal::from_str(candle[5].as_str().unwrap_or("0")).unwrap_or(Decimal::ZERO),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: BitoproBalanceResponse = self.private_request("GET", "/accounts/balance", None).await?;

        let mut currencies = HashMap::new();

        for b in response.data {
            let currency = b.currency.to_uppercase();
            let total = Decimal::from_str(&b.amount).unwrap_or(Decimal::ZERO);
            let available = Decimal::from_str(&b.available).unwrap_or(Decimal::ZERO);
            let used = total - available;

            currencies.insert(currency, Balance {
                free: Some(available),
                used: Some(used),
                total: Some(total),
                debt: None,
            });
        }

        Ok(Balances {
            info: serde_json::json!({}),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies,
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
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/orders/{market_id}");

        let action = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let type_str = match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            _ => return Err(CcxtError::BadRequest { message: "Unsupported order type".into() }),
        };

        let mut body = serde_json::json!({
            "action": action,
            "amount": amount.to_string(),
            "type": type_str,
        });

        if let Some(p) = price {
            body["price"] = serde_json::json!(p.to_string());
        }

        let response: BitoproCreateOrderResponse = self.private_request("POST", &path, Some(body)).await?;

        Ok(Order {
            id: response.order_id,
            client_order_id: None,
            datetime: Some(chrono::DateTime::from_timestamp_millis(response.timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            timestamp: Some(response.timestamp),
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
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(Decimal::ZERO),
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::json!({}),
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/orders/{market_id}/{id}");

        let _response: serde_json::Value = self.private_request("DELETE", &path, None).await?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            datetime: None,
            timestamp: None,
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
            remaining: Some(Decimal::ZERO),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(Decimal::ZERO),
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::json!({}),
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/orders/{market_id}/{id}");

        let response: BitoproOrderResponse = self.private_request("GET", &path, None).await?;

        let price = Decimal::from_str(&response.price).ok();
        let amount = Decimal::from_str(&response.original_amount).unwrap_or(Decimal::ZERO);
        let remaining = Decimal::from_str(&response.remaining_amount).unwrap_or(Decimal::ZERO);
        let filled = amount - remaining;
        let average = response.avg_execution_price.as_ref().and_then(|p| Decimal::from_str(p).ok());
        let cost = average.map(|a| a * filled);

        Ok(Order {
            id: response.order_id,
            client_order_id: None,
            datetime: Some(chrono::DateTime::from_timestamp_millis(response.timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            timestamp: Some(response.timestamp),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: Self::parse_order_status(response.status),
            symbol: symbol.to_string(),
            order_type: Self::parse_order_type(&response.order_type),
            time_in_force: None,
            side: Self::parse_order_side(&response.action),
            price,
            average,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::json!({}),
        })
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol required for fetching orders".into(),
        })?;
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let path = format!("/orders/all/{market_id}");
        let response: BitoproOrdersResponse = self.private_request("GET", &path, None).await?;

        let orders: Vec<Order> = response.data
            .iter()
            .map(|o| {
                let price = Decimal::from_str(&o.price).ok();
                let amount = Decimal::from_str(&o.original_amount).unwrap_or(Decimal::ZERO);
                let remaining = Decimal::from_str(&o.remaining_amount).unwrap_or(Decimal::ZERO);
                let filled = amount - remaining;
                let average = o.avg_execution_price.as_ref().and_then(|p| Decimal::from_str(p).ok());
                let cost = average.map(|a| a * filled);

                Order {
                    id: o.order_id.clone(),
                    client_order_id: None,
                    datetime: Some(chrono::DateTime::from_timestamp_millis(o.timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()),
                    timestamp: Some(o.timestamp),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: Self::parse_order_status(o.status),
                    symbol: symbol.to_string(),
                    order_type: Self::parse_order_type(&o.order_type),
                    time_in_force: None,
                    side: Self::parse_order_side(&o.action),
                    price,
                    average,
                    amount,
                    filled,
                    remaining: Some(remaining),
                    stop_price: None,
                    trigger_price: None,
                    take_profit_price: None,
                    stop_loss_price: None,
                    cost,
                    trades: vec![],
                    reduce_only: None,
                    post_only: None,
                    fee: None,
                    fees: vec![],
                    info: serde_json::json!({}),
                }
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::BadRequest {
            message: "Symbol required for fetching open orders".into(),
        })?;
        let market_id = self.get_market_id(symbol)?;
        let path = format!("/orders/{market_id}");

        let response: BitoproOrdersResponse = self.private_request("GET", &path, None).await?;

        let orders: Vec<Order> = response.data
            .iter()
            .filter(|o| o.status == 0 || o.status == 1) // Open or partially filled
            .map(|o| {
                let price = Decimal::from_str(&o.price).ok();
                let amount = Decimal::from_str(&o.original_amount).unwrap_or(Decimal::ZERO);
                let remaining = Decimal::from_str(&o.remaining_amount).unwrap_or(Decimal::ZERO);
                let filled = amount - remaining;

                Order {
                    id: o.order_id.clone(),
                    client_order_id: None,
                    datetime: Some(chrono::DateTime::from_timestamp_millis(o.timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()),
                    timestamp: Some(o.timestamp),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: symbol.to_string(),
                    order_type: Self::parse_order_type(&o.order_type),
                    time_in_force: None,
                    side: Self::parse_order_side(&o.action),
                    price,
                    average: None,
                    amount,
                    filled,
                    remaining: Some(remaining),
                    stop_price: None,
                    trigger_price: None,
                    take_profit_price: None,
                    stop_loss_price: None,
                    cost: Some(Decimal::ZERO),
                    trades: vec![],
                    reduce_only: None,
                    post_only: None,
                    fee: None,
                    fees: vec![],
                    info: serde_json::json!({}),
                }
            })
            .collect();

        Ok(orders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_exchange() -> Bitopro {
        let config = ExchangeConfig::default();
        Bitopro::new(config).unwrap()
    }

    #[test]
    fn test_bitopro_creation() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.id(), ExchangeId::Bitopro);
        assert_eq!(exchange.name(), "BitoPro");
    }

    #[test]
    fn test_exchange_info() {
        let exchange = create_test_exchange();
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().fetch_order_book);
        assert!(exchange.has().fetch_trades);
        assert!(exchange.has().fetch_ohlcv);
    }

    #[test]
    fn test_convert_to_market_id() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.convert_to_market_id("BTC/TWD"), "btc_twd");
        assert_eq!(exchange.convert_to_market_id("ETH/BTC"), "eth_btc");
    }

    #[test]
    fn test_convert_to_symbol() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.convert_to_symbol("btc_twd"), "BTC/TWD");
        assert_eq!(exchange.convert_to_symbol("eth_btc"), "ETH/BTC");
    }

    #[test]
    fn test_parse_order_status() {
        assert_eq!(Bitopro::parse_order_status(-1), OrderStatus::Canceled);
        assert_eq!(Bitopro::parse_order_status(0), OrderStatus::Open);
        assert_eq!(Bitopro::parse_order_status(1), OrderStatus::Open);
        assert_eq!(Bitopro::parse_order_status(2), OrderStatus::Closed);
        assert_eq!(Bitopro::parse_order_status(3), OrderStatus::Canceled);
    }

    #[test]
    fn test_parse_order_side() {
        assert_eq!(Bitopro::parse_order_side("buy"), OrderSide::Buy);
        assert_eq!(Bitopro::parse_order_side("BUY"), OrderSide::Buy);
        assert_eq!(Bitopro::parse_order_side("sell"), OrderSide::Sell);
        assert_eq!(Bitopro::parse_order_side("SELL"), OrderSide::Sell);
    }

    #[test]
    fn test_parse_order_type() {
        assert_eq!(Bitopro::parse_order_type("limit"), OrderType::Limit);
        assert_eq!(Bitopro::parse_order_type("LIMIT"), OrderType::Limit);
        assert_eq!(Bitopro::parse_order_type("market"), OrderType::Market);
        assert_eq!(Bitopro::parse_order_type("stop_limit"), OrderType::StopLimit);
    }

    #[test]
    fn test_features() {
        let exchange = create_test_exchange();
        let features = exchange.has();
        assert!(features.spot);
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_tickers);
        assert!(features.create_order);
        assert!(features.cancel_order);
    }

    #[test]
    fn test_timeframes() {
        let exchange = create_test_exchange();
        let timeframes = exchange.timeframes();
        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"1m".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"1h".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Day1), Some(&"1d".to_string()));
    }

    #[test]
    fn test_urls() {
        let exchange = create_test_exchange();
        let urls = exchange.urls();
        assert!(urls.www.as_ref().unwrap().contains("bitopro.com"));
        assert!(!urls.doc.is_empty());
    }
}
