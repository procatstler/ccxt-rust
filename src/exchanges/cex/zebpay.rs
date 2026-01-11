//! Zebpay Exchange Implementation
//!
//! Indian cryptocurrency exchange supporting spot and futures trading
//! API Docs: <https://github.com/zebpay/zebpay-api-references>

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Zebpay exchange
pub struct Zebpay {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayMarket {
    #[serde(rename = "tradePair")]
    trade_pair: String,
    #[serde(rename = "baseAsset")]
    base_asset: String,
    #[serde(rename = "quoteAsset")]
    quote_asset: String,
    #[serde(rename = "minTradeAmount")]
    min_trade_amount: Option<String>,
    #[serde(rename = "maxTradeAmount")]
    max_trade_amount: Option<String>,
    #[serde(rename = "tickSize")]
    tick_size: Option<String>,
    #[serde(rename = "stepSize")]
    step_size: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZebpayMarketsResponse {
    data: Vec<ZebpayMarket>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayTicker {
    #[serde(rename = "tradePair")]
    trade_pair: String,
    #[serde(rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(rename = "highPrice")]
    high_price: Option<String>,
    #[serde(rename = "lowPrice")]
    low_price: Option<String>,
    #[serde(rename = "volume")]
    volume: Option<String>,
    #[serde(rename = "quoteVolume")]
    quote_volume: Option<String>,
    #[serde(rename = "priceChange")]
    price_change: Option<String>,
    #[serde(rename = "priceChangePercent")]
    price_change_percent: Option<String>,
    #[serde(rename = "bidPrice")]
    bid_price: Option<String>,
    #[serde(rename = "askPrice")]
    ask_price: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ZebpayTickerResponse {
    data: ZebpayTicker,
}

#[derive(Debug, Deserialize)]
struct ZebpayTickersResponse {
    data: Vec<ZebpayTicker>,
}

#[derive(Debug, Deserialize)]
struct ZebpayOrderBookResponse {
    data: ZebpayOrderBookData,
}

#[derive(Debug, Deserialize)]
struct ZebpayOrderBookData {
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayTrade {
    id: Option<String>,
    price: String,
    #[serde(rename = "qty")]
    quantity: String,
    #[serde(rename = "quoteQty")]
    quote_qty: Option<String>,
    time: i64,
    #[serde(rename = "isBuyerMaker")]
    is_buyer_maker: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ZebpayTradeResponse {
    data: Vec<ZebpayTrade>,
}

#[derive(Debug, Deserialize)]
struct ZebpayOhlcvResponse {
    data: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayBalanceResponse {
    data: HashMap<String, ZebpayBalance>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayBalance {
    free: Option<String>,
    locked: Option<String>,
}

#[derive(Debug, Serialize)]
struct ZebpayOrderRequest {
    symbol: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "quoteOrderQty")]
    quote_order_qty: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayOrderResponse {
    data: ZebpayOrder,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZebpayOrder {
    #[serde(rename = "orderId")]
    order_id: String,
    symbol: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    price: Option<String>,
    #[serde(rename = "origQty")]
    orig_qty: Option<String>,
    #[serde(rename = "executedQty")]
    executed_qty: Option<String>,
    status: String,
    #[serde(rename = "transactTime")]
    transact_time: Option<i64>,
    #[serde(rename = "updateTime")]
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ZebpayOrdersResponse {
    data: Vec<ZebpayOrder>,
}

impl Zebpay {
    const BASE_URL: &'static str = "https://sapi.zebpay.com";
    const FUTURES_URL: &'static str = "https://futuresbe.zebpay.com";
    const RATE_LIMIT_MS: u64 = 20; // 50 requests per second

    /// Create new Zebpay instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: false,
            swap: true,
            future: true,
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
            cancel_all_orders: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("spot".into(), Self::BASE_URL.into());
        api_urls.insert("futures".into(), Self::FUTURES_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/120467903-29f14500-c37e-11eb-818f-68c72f1d1cbe.jpg".into()),
            api: api_urls,
            www: Some("https://www.zebpay.com".into()),
            doc: vec![
                "https://github.com/zebpay/zebpay-api-references".into(),
                "https://build.zebpay.com/".into(),
            ],
            fees: Some("https://zebpay.com/in/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".to_string());
        timeframes.insert(Timeframe::Minute3, "3m".to_string());
        timeframes.insert(Timeframe::Minute5, "5m".to_string());
        timeframes.insert(Timeframe::Minute15, "15m".to_string());
        timeframes.insert(Timeframe::Minute30, "30m".to_string());
        timeframes.insert(Timeframe::Hour1, "1h".to_string());
        timeframes.insert(Timeframe::Hour2, "2h".to_string());
        timeframes.insert(Timeframe::Hour4, "4h".to_string());
        timeframes.insert(Timeframe::Hour6, "6h".to_string());
        timeframes.insert(Timeframe::Hour12, "12h".to_string());
        timeframes.insert(Timeframe::Day1, "1d".to_string());
        timeframes.insert(Timeframe::Week1, "1w".to_string());

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

    /// Sign request with HMAC-SHA256
    fn sign_payload(&self, payload: &str) -> CcxtResult<String> {
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret required".into(),
        })?;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(payload.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Public API request
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let url = format!("/api/v2{path}");
        self.client.get(&url, params, None).await
    }

    /// Private API request
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;

        let url = format!("/api/v2{path}");

        // Build query string or body for signing
        let payload = if let Some(b) = body {
            b.to_string()
        } else if let Some(ref p) = params {
            let mut sorted: Vec<_> = p.iter().collect();
            sorted.sort_by_key(|&(k, _)| k);
            sorted.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        } else {
            String::new()
        };

        let signature = self.sign_payload(&payload)?;

        let mut headers = HashMap::new();
        headers.insert("X-AUTH-APIKEY".into(), api_key.to_string());
        headers.insert("X-AUTH-SIGNATURE".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => self.client.get(&url, params, Some(headers)).await,
            "POST" => {
                let body_value: Option<serde_json::Value> = body
                    .map(|b| serde_json::from_str(b).unwrap_or(serde_json::json!({})));
                self.client.post(&url, body_value, Some(headers)).await
            },
            "DELETE" => self.client.delete(&url, params, Some(headers)).await,
            _ => Err(CcxtError::ExchangeError { message: format!("Unknown method: {method}") }),
        }
    }

    /// Convert exchange market ID to unified symbol
    fn parse_symbol(&self, market_id: &str) -> String {
        let markets = self.markets_by_id.read().unwrap();
        if let Some(symbol) = markets.get(market_id) {
            symbol.clone()
        } else {
            // Try to parse from format like "BTC-INR" or "BTCINR"
            if market_id.contains('-') {
                let parts: Vec<&str> = market_id.split('-').collect();
                if parts.len() == 2 {
                    return format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase());
                }
            }
            market_id.to_uppercase()
        }
    }

    /// Convert unified symbol to exchange market ID
    fn format_symbol(&self, symbol: &str) -> String {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            market.id.clone()
        } else {
            symbol.replace('/', "-").to_uppercase()
        }
    }

    /// Get market ID from symbol
    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            Ok(market.id.clone())
        } else {
            Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })
        }
    }

    /// Parse order status
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status.to_uppercase().as_str() {
            "NEW" | "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "CANCELLED" | "REJECTED" | "EXPIRED" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// Parse order side
    fn parse_order_side(&self, side: &str) -> OrderSide {
        match side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    /// Parse order type
    fn parse_order_type(&self, order_type: &str) -> OrderType {
        match order_type.to_uppercase().as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP_LOSS" => OrderType::StopLoss,
            "STOP_LOSS_LIMIT" => OrderType::StopLossLimit,
            "TAKE_PROFIT" => OrderType::TakeProfit,
            "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
            _ => OrderType::Limit,
        }
    }

    /// Parse order from response
    fn parse_order(&self, order: &ZebpayOrder) -> Order {
        let symbol = self.parse_symbol(&order.symbol);
        let amount = order.orig_qty.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let filled = order.executed_qty.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let remaining = amount - filled;

        Order {
            id: order.order_id.clone(),
            client_order_id: None,
            timestamp: order.transact_time,
            datetime: order.transact_time.map(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            last_trade_timestamp: None,
            last_update_timestamp: order.update_time,
            status: self.parse_order_status(&order.status),
            symbol,
            order_type: self.parse_order_type(&order.order_type),
            time_in_force: None,
            side: self.parse_order_side(&order.side),
            price: order.price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            average: None,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: vec![],
            fee: None,
            fees: vec![],
            reduce_only: None,
            post_only: None,
            info: serde_json::json!(order),
        }
    }
}

#[async_trait]
impl Exchange for Zebpay {
    fn id(&self) -> ExchangeId {
        ExchangeId::Zebpay
    }

    fn name(&self) -> &str {
        "Zebpay"
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
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: ZebpayMarketsResponse = self.public_get("/markets", None).await?;

        let mut markets = Vec::new();
        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for m in response.data {
            let symbol = format!("{}/{}", m.base_asset.to_uppercase(), m.quote_asset.to_uppercase());
            let market_id = m.trade_pair.clone();

            let tick_size = m.tick_size.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::new(1, 8));
            let step_size = m.step_size.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::new(1, 8));

            let price_precision = tick_size.scale() as i32;
            let amount_precision = step_size.scale() as i32;

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: m.base_asset.to_uppercase(),
                quote: m.quote_asset.to_uppercase(),
                base_id: m.base_asset.clone(),
                quote_id: m.quote_asset.clone(),
                active: m.status.as_ref().map(|s| s == "TRADING").unwrap_or(true),
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
                settle: None,
                settle_id: None,
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                maker: Some(Decimal::new(1, 3)), // 0.1%
                taker: Some(Decimal::new(1, 3)), // 0.1%
                sub_type: None,
                margin_modes: None,
                tier_based: false,
                percentage: true,
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: m.min_trade_amount.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                        max: m.max_trade_amount.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                    },
                    price: crate::types::MinMax { min: None, max: None },
                    cost: crate::types::MinMax { min: None, max: None },
                    leverage: crate::types::MinMax { min: None, max: None },
                },
                created: None,
                info: serde_json::json!(m),
            };

            markets_map.insert(symbol.clone(), market.clone());
            markets_by_id_map.insert(market_id, symbol);
            markets.push(market);
        }

        // Update cached markets
        {
            let mut cache = self.markets.write().unwrap();
            *cache = markets_map;
        }
        {
            let mut cache = self.markets_by_id.write().unwrap();
            *cache = markets_by_id_map;
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: ZebpayTickerResponse = self.public_get("/ticker/24hr", Some(params)).await?;
        let t = response.data;

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: t.timestamp,
            datetime: t.timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            high: t.high_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            low: t.low_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid: t.bid_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: None,
            ask: t.ask_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: t.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            last: t.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: t.price_change.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            percentage: t.price_change_percent.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            average: None,
            base_volume: t.volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: t.quote_volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::json!(t),
        })
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: ZebpayTickersResponse = self.public_get("/ticker/24hr", None).await?;

        let mut tickers: HashMap<String, Ticker> = response.data.iter().map(|t| {
            let symbol = self.parse_symbol(&t.trade_pair);
            let ticker = Ticker {
                symbol: symbol.clone(),
                timestamp: t.timestamp,
                datetime: t.timestamp.map(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                }),
                high: t.high_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                low: t.low_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                bid: t.bid_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                bid_volume: None,
                ask: t.ask_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                ask_volume: None,
                vwap: None,
                open: None,
                close: t.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                last: t.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                previous_close: None,
                change: t.price_change.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                percentage: t.price_change_percent.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                average: None,
                base_volume: t.volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                quote_volume: t.quote_volume.as_ref().and_then(|s| Decimal::from_str(s).ok()),
                index_price: None,
                mark_price: None,
                info: serde_json::json!(t),
            };
            (symbol, ticker)
        }).collect();

        if let Some(syms) = symbols {
            let sym_set: std::collections::HashSet<&str> = syms.iter().copied().collect();
            tickers.retain(|k, _| sym_set.contains(k.as_str()));
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ZebpayOrderBookResponse = self.public_get("/depth", Some(params)).await?;
        let data = response.data;

        let parse_entry = |entry: &Vec<String>| -> Option<OrderBookEntry> {
            if entry.len() >= 2 {
                let price = Decimal::from_str(&entry[0]).ok()?;
                let amount = Decimal::from_str(&entry[1]).ok()?;
                Some(OrderBookEntry { price, amount })
            } else {
                None
            }
        };

        let bids: Vec<OrderBookEntry> = data.bids.iter().filter_map(parse_entry).collect();
        let asks: Vec<OrderBookEntry> = data.asks.iter().filter_map(parse_entry).collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: data.timestamp,
            datetime: data.timestamp.map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            }),
            nonce: None,
        })
    }

    async fn fetch_trades(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ZebpayTradeResponse = self.public_get("/trades", Some(params)).await?;

        let mut trades: Vec<Trade> = response.data.iter().map(|t| {
            let price = Decimal::from_str(&t.price).unwrap_or(Decimal::ZERO);
            let amount = Decimal::from_str(&t.quantity).unwrap_or(Decimal::ZERO);
            let cost = price * amount;
            let timestamp = t.time;

            Trade {
                id: t.id.clone().unwrap_or_default(),
                order: None,
                timestamp: Some(timestamp),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(if t.is_buyer_maker.unwrap_or(false) { "sell".to_string() } else { "buy".to_string() }),
                taker_or_maker: None,
                price,
                amount,
                cost: Some(cost),
                fee: None,
                fees: Vec::new(),
                info: serde_json::json!(t),
            }
        }).collect();

        if let Some(s) = since {
            trades.retain(|t| t.timestamp.unwrap_or(0) >= s);
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.format_symbol(symbol);
        let interval = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::BadSymbol { symbol: format!("Unsupported timeframe: {timeframe:?}") })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ZebpayOhlcvResponse = self.public_get("/klines", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response.data.iter().filter_map(|candle| {
            if candle.len() >= 6 {
                Some(OHLCV {
                    timestamp: candle[0].as_i64()?,
                    open: candle[1].as_str().and_then(|s| Decimal::from_str(s).ok())?,
                    high: candle[2].as_str().and_then(|s| Decimal::from_str(s).ok())?,
                    low: candle[3].as_str().and_then(|s| Decimal::from_str(s).ok())?,
                    close: candle[4].as_str().and_then(|s| Decimal::from_str(s).ok())?,
                    volume: candle[5].as_str().and_then(|s| Decimal::from_str(s).ok())?,
                })
            } else {
                None
            }
        }).collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: ZebpayBalanceResponse = self.private_request("GET", "/account", None, None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());
        balances.info = serde_json::json!(response);

        for (currency, bal) in response.data {
            let free = bal.free.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let used = bal.locked.as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or(Decimal::ZERO);
            let total = free + used;

            if total > Decimal::ZERO {
                balances.currencies.insert(currency.to_uppercase(), Balance {
                    free: Some(free),
                    used: Some(used),
                    total: Some(total),
                    debt: None,
                });
            }
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
        let market_id = self.format_symbol(symbol);

        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let type_str = match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            _ => return Err(CcxtError::NotSupported { feature: format!("{order_type:?} orders") }),
        };

        let request = ZebpayOrderRequest {
            symbol: market_id,
            side: side_str.to_string(),
            order_type: type_str.to_string(),
            quantity: Some(amount.to_string()),
            price: price.map(|p| p.to_string()),
            quote_order_qty: None,
        };

        let body = serde_json::to_string(&request)
            .map_err(|e| CcxtError::ExchangeError { message: e.to_string() })?;

        let response: ZebpayOrderResponse = self.private_request("POST", "/order", None, Some(&body)).await?;

        Ok(self.parse_order(&response.data))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: ZebpayOrderResponse = self.private_request("DELETE", "/order", Some(params), None).await?;

        Ok(self.parse_order(&response.data))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: ZebpayOrderResponse = self.private_request("GET", "/order", Some(params), None).await?;

        Ok(self.parse_order(&response.data))
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("symbol".into(), self.format_symbol(sym));
        }

        let response: ZebpayOrdersResponse = self.private_request("GET", "/openOrders", Some(params), None).await?;

        Ok(response.data.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("symbol".into(), self.format_symbol(sym));
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ZebpayOrdersResponse = self.private_request("GET", "/allOrders", Some(params), None).await?;

        Ok(response.data.iter()
            .filter(|o| o.status == "FILLED" || o.status == "CANCELED")
            .map(|o| self.parse_order(o))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zebpay_creation() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config);
        assert!(exchange.is_ok());
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Zebpay);
        assert_eq!(exchange.name(), "Zebpay");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_parse_symbol() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();

        assert_eq!(exchange.parse_symbol("BTC-INR"), "BTC/INR");
        assert_eq!(exchange.parse_symbol("ETH-USDT"), "ETH/USDT");
    }

    #[test]
    fn test_format_symbol() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();

        assert_eq!(exchange.format_symbol("BTC/INR"), "BTC-INR");
        assert_eq!(exchange.format_symbol("ETH/USDT"), "ETH-USDT");
    }

    #[test]
    fn test_parse_order_status() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();

        assert_eq!(exchange.parse_order_status("NEW"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("PARTIALLY_FILLED"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("FILLED"), OrderStatus::Closed);
        assert_eq!(exchange.parse_order_status("CANCELED"), OrderStatus::Canceled);
    }

    #[test]
    fn test_parse_order_side() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();

        assert_eq!(exchange.parse_order_side("BUY"), OrderSide::Buy);
        assert_eq!(exchange.parse_order_side("SELL"), OrderSide::Sell);
    }

    #[test]
    fn test_parse_order_type() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();

        assert_eq!(exchange.parse_order_type("LIMIT"), OrderType::Limit);
        assert_eq!(exchange.parse_order_type("MARKET"), OrderType::Market);
    }

    #[test]
    fn test_features() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();
        let features = exchange.has();

        assert!(features.spot);
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.fetch_order_book);
        assert!(features.create_order);
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Zebpay::new(config).unwrap();
        let timeframes = exchange.timeframes();

        assert!(timeframes.contains_key(&Timeframe::Minute1));
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
    }
}
