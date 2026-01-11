//! CoinsPH Exchange Implementation
//!
//! Philippine cryptocurrency exchange with Binance-like API structure
//! API Docs: <https://coins-docs.github.io/rest-api>

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// CoinsPH exchange (Philippines)
pub struct Coinsph {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

// === API Response Types ===

#[derive(Debug, Deserialize)]
struct CoinsphExchangeInfo {
    timezone: Option<String>,
    symbols: Vec<CoinsphSymbol>,
    #[serde(rename = "rateLimits")]
    rate_limits: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinsphSymbol {
    symbol: String,
    status: Option<String>,
    #[serde(rename = "baseAsset")]
    base_asset: String,
    #[serde(rename = "baseAssetPrecision")]
    base_asset_precision: Option<i32>,
    #[serde(rename = "quoteAsset")]
    quote_asset: String,
    #[serde(rename = "quotePrecision")]
    quote_precision: Option<i32>,
    #[serde(rename = "quoteAssetPrecision")]
    quote_asset_precision: Option<i32>,
    filters: Option<Vec<CoinsphFilter>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinsphFilter {
    #[serde(rename = "filterType")]
    filter_type: String,
    #[serde(rename = "minPrice")]
    min_price: Option<String>,
    #[serde(rename = "maxPrice")]
    max_price: Option<String>,
    #[serde(rename = "tickSize")]
    tick_size: Option<String>,
    #[serde(rename = "minQty")]
    min_qty: Option<String>,
    #[serde(rename = "maxQty")]
    max_qty: Option<String>,
    #[serde(rename = "stepSize")]
    step_size: Option<String>,
    #[serde(rename = "minNotional")]
    min_notional: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinsphTicker {
    symbol: String,
    #[serde(rename = "priceChange")]
    price_change: Option<String>,
    #[serde(rename = "priceChangePercent")]
    price_change_percent: Option<String>,
    #[serde(rename = "lastPrice")]
    last_price: Option<String>,
    #[serde(rename = "bidPrice")]
    bid_price: Option<String>,
    #[serde(rename = "bidQty")]
    bid_qty: Option<String>,
    #[serde(rename = "askPrice")]
    ask_price: Option<String>,
    #[serde(rename = "askQty")]
    ask_qty: Option<String>,
    #[serde(rename = "openPrice")]
    open_price: Option<String>,
    #[serde(rename = "highPrice")]
    high_price: Option<String>,
    #[serde(rename = "lowPrice")]
    low_price: Option<String>,
    volume: Option<String>,
    #[serde(rename = "quoteVolume")]
    quote_volume: Option<String>,
    #[serde(rename = "openTime")]
    open_time: Option<i64>,
    #[serde(rename = "closeTime")]
    close_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoinsphOrderBook {
    #[serde(rename = "lastUpdateId")]
    last_update_id: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CoinsphTrade {
    id: Option<i64>,
    price: String,
    qty: String,
    #[serde(rename = "quoteQty")]
    quote_qty: Option<String>,
    time: i64,
    #[serde(rename = "isBuyerMaker")]
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinsphBalance {
    asset: String,
    free: String,
    locked: String,
}

#[derive(Debug, Deserialize)]
struct CoinsphAccountInfo {
    balances: Vec<CoinsphBalance>,
    #[serde(rename = "canTrade")]
    can_trade: Option<bool>,
    #[serde(rename = "canWithdraw")]
    can_withdraw: Option<bool>,
    #[serde(rename = "canDeposit")]
    can_deposit: Option<bool>,
    #[serde(rename = "updateTime")]
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinsphOrder {
    symbol: String,
    #[serde(rename = "orderId")]
    order_id: i64,
    #[serde(rename = "clientOrderId")]
    client_order_id: Option<String>,
    #[serde(rename = "transactTime")]
    transact_time: Option<i64>,
    price: Option<String>,
    #[serde(rename = "origQty")]
    orig_qty: Option<String>,
    #[serde(rename = "executedQty")]
    executed_qty: Option<String>,
    #[serde(rename = "cummulativeQuoteQty")]
    cummulative_quote_qty: Option<String>,
    status: Option<String>,
    #[serde(rename = "timeInForce")]
    time_in_force: Option<String>,
    #[serde(rename = "type")]
    order_type: Option<String>,
    side: Option<String>,
    time: Option<i64>,
    #[serde(rename = "updateTime")]
    update_time: Option<i64>,
    #[serde(rename = "avgPrice")]
    avg_price: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoinsphMyTrade {
    id: i64,
    #[serde(rename = "orderId")]
    order_id: i64,
    symbol: String,
    price: String,
    qty: String,
    #[serde(rename = "quoteQty")]
    quote_qty: Option<String>,
    commission: Option<String>,
    #[serde(rename = "commissionAsset")]
    commission_asset: Option<String>,
    time: i64,
    #[serde(rename = "isBuyer")]
    is_buyer: bool,
    #[serde(rename = "isMaker")]
    is_maker: bool,
}

impl Coinsph {
    const BASE_URL: &'static str = "https://api.pro.coins.ph";
    const RATE_LIMIT_MS: u64 = 100; // 600 requests/min = 10/sec

    /// Create a new CoinsPH exchange instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://coins.ph/favicon.ico".to_string()),
            api: api_urls,
            www: Some("https://pro.coins.ph".to_string()),
            doc: vec!["https://coins-docs.github.io/rest-api".to_string()],
            fees: None,
        };

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
            fetch_my_trades: true,
            ..Default::default()
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

    fn generate_signature(&self, query_string: &str) -> CcxtResult<String> {
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API secret not configured".into(),
        })?;

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|e| CcxtError::AuthenticationError { message: e.to_string() })?;
        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        Ok(signature)
    }

    async fn public_get<T: serde::de::DeserializeOwned>(&self, path: &str, params: Option<&HashMap<String, String>>) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let url = if let Some(p) = params {
            if !p.is_empty() {
                let query: String = p.iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                format!("{}{}", path, if path.contains('?') { "&" } else { "?" }.to_string() + &query)
            } else {
                path.to_string()
            }
        } else {
            path.to_string()
        };

        self.client.get(&url, None, None).await
    }

    async fn private_request<T: serde::de::DeserializeOwned>(&self, method: &str, path: &str, params: Option<HashMap<String, String>>) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key not configured".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();
        let mut query_params = params.unwrap_or_default();
        query_params.insert("timestamp".to_string(), timestamp.to_string());

        // Build sorted query string
        let mut sorted_params: Vec<_> = query_params.iter().collect();
        sorted_params.sort_by(|a, b| a.0.cmp(b.0));

        let query_string: String = sorted_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let signature = self.generate_signature(&query_string)?;

        let mut headers = HashMap::new();
        headers.insert("X-COINS-APIKEY".to_string(), api_key.to_string());

        match method {
            "GET" => {
                let url = format!("{path}?{query_string}&signature={signature}");
                self.client.get(&url, None, Some(headers)).await
            }
            "POST" => {
                headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string());
                let body_str = format!("{query_string}&signature={signature}");
                let body = serde_json::json!(body_str);
                self.client.post(path, Some(body), Some(headers)).await
            }
            "DELETE" => {
                let url = format!("{path}?{query_string}&signature={signature}");
                self.client.delete(&url, None, Some(headers)).await
            }
            _ => Err(CcxtError::BadRequest { message: format!("Unsupported HTTP method: {method}") })
        }
    }

    fn get_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        if let Some(market) = markets.get(symbol) {
            return Ok(market.id.clone());
        }
        // Default conversion: remove slash
        Ok(symbol.replace('/', ""))
    }

    fn get_symbol(&self, market_id: &str) -> String {
        let markets_by_id = self.markets_by_id.read().unwrap();
        if let Some(symbol) = markets_by_id.get(market_id) {
            return symbol.clone();
        }
        market_id.to_string()
    }

    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status.to_uppercase().as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "PENDING_CANCEL" => OrderStatus::Open,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        }
    }

    fn parse_order_side(&self, side: &str) -> OrderSide {
        match side.to_uppercase().as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        }
    }

    fn parse_order_type(&self, order_type: &str) -> OrderType {
        match order_type.to_uppercase().as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "STOP_LOSS" => OrderType::StopLoss,
            "STOP_LOSS_LIMIT" => OrderType::StopLossLimit,
            "TAKE_PROFIT" => OrderType::TakeProfit,
            "TAKE_PROFIT_LIMIT" => OrderType::TakeProfitLimit,
            "LIMIT_MAKER" => OrderType::Limit,
            _ => OrderType::Limit,
        }
    }

    fn count_decimals(&self, value: &str) -> i32 {
        if let Some(pos) = value.find('.') {
            let decimal_part = &value[pos + 1..].trim_end_matches('0');
            decimal_part.len() as i32
        } else {
            0
        }
    }

    fn parse_ticker(&self, ticker: &CoinsphTicker) -> CcxtResult<Ticker> {
        let symbol = self.get_symbol(&ticker.symbol);
        let timestamp = ticker.close_time.unwrap_or_else(|| Utc::now().timestamp_millis());

        let last = ticker.last_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let high = ticker.high_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let low = ticker.low_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let open = ticker.open_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let bid = ticker.bid_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let bid_volume = ticker.bid_qty.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let ask = ticker.ask_price.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());
        let ask_volume = ticker.ask_qty.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let base_volume = ticker.volume.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let quote_volume = ticker.quote_volume.as_ref()
            .and_then(|v| Decimal::from_str(v).ok());
        let change = ticker.price_change.as_ref()
            .and_then(|c| Decimal::from_str(c).ok());
        let percentage = ticker.price_change_percent.as_ref()
            .and_then(|p| Decimal::from_str(p).ok());

        let vwap = if let (Some(qv), Some(bv)) = (quote_volume, base_volume) {
            if bv > Decimal::ZERO {
                Some(qv / bv)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Ticker {
            symbol,
            timestamp: Some(timestamp),
            datetime: Some(chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            high,
            low,
            bid,
            bid_volume,
            ask,
            ask_volume,
            vwap,
            open,
            close: last,
            last,
            previous_close: None,
            change,
            percentage,
            average: None,
            base_volume,
            quote_volume,
            info: serde_json::json!({}),
            index_price: None,
            mark_price: None,
        })
    }
}

#[async_trait]
impl Exchange for Coinsph {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinsph
    }

    fn name(&self) -> &str {
        "CoinsPH"
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

        let response: CoinsphExchangeInfo = self.public_get("/openapi/v1/exchangeInfo", None).await?;

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for symbol_info in response.symbols {
            let base = symbol_info.base_asset.to_uppercase();
            let quote = symbol_info.quote_asset.to_uppercase();
            let symbol = format!("{base}/{quote}");
            let market_id = symbol_info.symbol.clone();

            let active = symbol_info.status.as_ref().map(|s| s == "TRADING").unwrap_or(true);

            let base_precision = symbol_info.base_asset_precision.unwrap_or(8);
            let quote_precision = symbol_info.quote_asset_precision
                .or(symbol_info.quote_precision)
                .unwrap_or(8);

            let mut min_price = None;
            let mut max_price = None;
            let mut min_amount = None;
            let mut max_amount = None;
            let mut min_cost = None;
            let mut price_precision = quote_precision;
            let mut amount_precision = base_precision;

            if let Some(filters) = &symbol_info.filters {
                for filter in filters {
                    match filter.filter_type.as_str() {
                        "PRICE_FILTER" => {
                            if let Some(mp) = &filter.min_price {
                                min_price = Decimal::from_str(mp).ok();
                            }
                            if let Some(mp) = &filter.max_price {
                                max_price = Decimal::from_str(mp).ok();
                            }
                            if let Some(ts) = &filter.tick_size {
                                price_precision = self.count_decimals(ts);
                            }
                        }
                        "LOT_SIZE" => {
                            if let Some(mq) = &filter.min_qty {
                                min_amount = Decimal::from_str(mq).ok();
                            }
                            if let Some(mq) = &filter.max_qty {
                                max_amount = Decimal::from_str(mq).ok();
                            }
                            if let Some(ss) = &filter.step_size {
                                amount_precision = self.count_decimals(ss);
                            }
                        }
                        "MIN_NOTIONAL" => {
                            if let Some(mn) = &filter.min_notional {
                                min_cost = Decimal::from_str(mn).ok();
                            }
                        }
                        _ => {}
                    }
                }
            }

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                active,
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
                taker: Some(Decimal::from_str("0.001").unwrap()),
                maker: Some(Decimal::from_str("0.001").unwrap()),
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: Some(quote_precision),
                    base: Some(base_precision),
                    quote: Some(quote_precision),
                },
                limits: MarketLimits {
                    amount: crate::types::MinMax {
                        min: min_amount,
                        max: max_amount,
                    },
                    price: crate::types::MinMax {
                        min: min_price,
                        max: max_price,
                    },
                    cost: crate::types::MinMax {
                        min: min_cost,
                        max: None,
                    },
                    leverage: crate::types::MinMax {
                        min: None,
                        max: None,
                    },
                },
                info: serde_json::to_value(&symbol_info).unwrap_or_default(),
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
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: CoinsphTicker = self.public_get("/openapi/quote/v1/ticker/24hr", Some(&params)).await?;
        self.parse_ticker(&response)
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<CoinsphTicker> = self.public_get("/openapi/quote/v1/ticker/24hr", None).await?;

        let mut tickers = HashMap::new();
        for ticker_data in response {
            if let Ok(ticker) = self.parse_ticker(&ticker_data) {
                if let Some(syms) = symbols {
                    if syms.iter().any(|&s| s == ticker.symbol) {
                        tickers.insert(ticker.symbol.clone(), ticker);
                    }
                } else {
                    tickers.insert(ticker.symbol.clone(), ticker);
                }
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: CoinsphOrderBook = self.public_get("/openapi/quote/v1/depth", Some(&params)).await?;

        let mut bids = Vec::new();
        let mut asks = Vec::new();

        for bid in &response.bids {
            if bid.len() >= 2 {
                bids.push(OrderBookEntry {
                    price: Decimal::from_str(&bid[0]).unwrap_or(Decimal::ZERO),
                    amount: Decimal::from_str(&bid[1]).unwrap_or(Decimal::ZERO),
                });
            }
        }

        for ask in &response.asks {
            if ask.len() >= 2 {
                asks.push(OrderBookEntry {
                    price: Decimal::from_str(&ask[0]).unwrap_or(Decimal::ZERO),
                    amount: Decimal::from_str(&ask[1]).unwrap_or(Decimal::ZERO),
                });
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: response.last_update_id,
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.get_market_id(symbol)?;
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: Vec<CoinsphTrade> = self.public_get("/openapi/quote/v1/trades", Some(&params)).await?;

        let mut trades = Vec::new();
        for trade_data in response {
            let price = Decimal::from_str(&trade_data.price).unwrap_or(Decimal::ZERO);
            let amount = Decimal::from_str(&trade_data.qty).unwrap_or(Decimal::ZERO);
            let cost = price * amount;

            let side = if trade_data.is_buyer_maker {
                "sell"
            } else {
                "buy"
            };

            trades.push(Trade {
                id: trade_data.id.map(|id| id.to_string()).unwrap_or_default(),
                order: None,
                info: serde_json::json!({}),
                timestamp: Some(trade_data.time),
                datetime: Some(chrono::DateTime::from_timestamp_millis(trade_data.time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()),
                symbol: symbol.to_string(),
                trade_type: None,
                taker_or_maker: None,
                side: Some(side.to_string()),
                price,
                amount,
                cost: Some(cost),
                fee: None,
                fees: vec![],
            });
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
            .ok_or_else(|| CcxtError::BadSymbol { symbol: format!("Timeframe {timeframe:?} not supported") })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), tf.clone());

        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: Vec<Vec<serde_json::Value>> = self.public_get("/openapi/quote/v1/klines", Some(&params)).await?;

        let mut ohlcv = Vec::new();
        for kline in response {
            if kline.len() >= 6 {
                let timestamp = kline[0].as_i64().unwrap_or(0);
                let open = kline[1].as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let high = kline[2].as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let low = kline[3].as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let close = kline[4].as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let volume = kline[5].as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);

                ohlcv.push(OHLCV {
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume,
                });
            }
        }

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: CoinsphAccountInfo = self.private_request("GET", "/openapi/v1/account", None).await?;

        let mut currencies = HashMap::new();

        for balance_data in response.balances {
            let free = Decimal::from_str(&balance_data.free).unwrap_or(Decimal::ZERO);
            let used = Decimal::from_str(&balance_data.locked).unwrap_or(Decimal::ZERO);
            let total = free + used;

            if total > Decimal::ZERO || free > Decimal::ZERO || used > Decimal::ZERO {
                currencies.insert(
                    balance_data.asset.to_uppercase(),
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
        }

        Ok(Balances {
            info: serde_json::Value::Null,
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

        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let type_str = match order_type {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
            OrderType::StopLoss => "STOP_LOSS",
            OrderType::StopLossLimit => "STOP_LOSS_LIMIT",
            OrderType::TakeProfit => "TAKE_PROFIT",
            OrderType::TakeProfitLimit => "TAKE_PROFIT_LIMIT",
            _ => "LIMIT",
        };

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("side".to_string(), side_str.to_string());
        params.insert("type".to_string(), type_str.to_string());
        params.insert("quantity".to_string(), amount.to_string());

        if let Some(p) = price {
            params.insert("price".to_string(), p.to_string());
            if order_type == OrderType::Limit {
                params.insert("timeInForce".to_string(), "GTC".to_string());
            }
        }

        let response: CoinsphOrder = self.private_request("POST", "/openapi/v1/order", Some(params)).await?;

        let filled = response.executed_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or(Decimal::ZERO);

        let remaining = response.orig_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or(amount) - filled;

        let avg_price = response.avg_price
            .as_ref()
            .and_then(|p| Decimal::from_str(p).ok());

        let cost = response.cummulative_quote_qty
            .as_ref()
            .and_then(|c| Decimal::from_str(c).ok())
            .unwrap_or(Decimal::ZERO);

        Ok(Order {
            id: response.order_id.to_string(),
            client_order_id: response.client_order_id.clone(),
            datetime: Some(response.transact_time
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            timestamp: Some(response.transact_time.unwrap_or_else(|| Utc::now().timestamp_millis())),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: response.status
                .as_ref()
                .map(|s| self.parse_order_status(s))
                .unwrap_or(OrderStatus::Open),
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: price.or_else(|| response.price.as_ref().and_then(|p| Decimal::from_str(p).ok())),
            average: avg_price,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(cost),
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("orderId".to_string(), id.to_string());

        let response: CoinsphOrder = self.private_request("DELETE", "/openapi/v1/order", Some(params)).await?;

        let amount = response.orig_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or(Decimal::ZERO);

        let filled = response.executed_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or(Decimal::ZERO);

        Ok(Order {
            id: response.order_id.to_string(),
            client_order_id: response.client_order_id.clone(),
            datetime: Some(response.time
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            timestamp: Some(response.time.unwrap_or_else(|| Utc::now().timestamp_millis())),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: response.order_type
                .as_ref()
                .map(|t| self.parse_order_type(t))
                .unwrap_or(OrderType::Limit),
            time_in_force: None,
            side: response.side
                .as_ref()
                .map(|s| self.parse_order_side(s))
                .unwrap_or(OrderSide::Buy),
            price: response.price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            average: response.avg_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
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
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.get_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("orderId".to_string(), id.to_string());

        let response: CoinsphOrder = self.private_request("GET", "/openapi/v1/order", Some(params)).await?;

        let amount = response.orig_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or(Decimal::ZERO);

        let filled = response.executed_qty
            .as_ref()
            .and_then(|q| Decimal::from_str(q).ok())
            .unwrap_or(Decimal::ZERO);

        let cost = response.cummulative_quote_qty
            .as_ref()
            .and_then(|c| Decimal::from_str(c).ok())
            .unwrap_or(Decimal::ZERO);

        Ok(Order {
            id: response.order_id.to_string(),
            client_order_id: response.client_order_id.clone(),
            datetime: Some(response.time
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()),
            timestamp: Some(response.time.unwrap_or_else(|| Utc::now().timestamp_millis())),
            last_trade_timestamp: response.update_time,
            last_update_timestamp: None,
            status: response.status
                .as_ref()
                .map(|s| self.parse_order_status(s))
                .unwrap_or(OrderStatus::Open),
            symbol: symbol.to_string(),
            order_type: response.order_type
                .as_ref()
                .map(|t| self.parse_order_type(t))
                .unwrap_or(OrderType::Limit),
            time_in_force: None,
            side: response.side
                .as_ref()
                .map(|s| self.parse_order_side(s))
                .unwrap_or(OrderSide::Buy),
            price: response.price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            average: response.avg_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: Some(cost),
            trades: vec![],
            reduce_only: None,
            post_only: None,
            fee: None,
            fees: vec![],
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_orders(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".to_string(), self.get_market_id(s)?);
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: Vec<CoinsphOrder> = self.private_request("GET", "/openapi/v1/historyOrders", Some(params)).await?;

        let mut orders = Vec::new();
        for order_data in response {
            let sym = self.get_symbol(&order_data.symbol);
            let amount = order_data.orig_qty
                .as_ref()
                .and_then(|q| Decimal::from_str(q).ok())
                .unwrap_or(Decimal::ZERO);

            let filled = order_data.executed_qty
                .as_ref()
                .and_then(|q| Decimal::from_str(q).ok())
                .unwrap_or(Decimal::ZERO);

            let cost = order_data.cummulative_quote_qty
                .as_ref()
                .and_then(|c| Decimal::from_str(c).ok())
                .unwrap_or(Decimal::ZERO);

            orders.push(Order {
                id: order_data.order_id.to_string(),
                client_order_id: order_data.client_order_id.clone(),
                datetime: Some(order_data.time
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()),
                timestamp: Some(order_data.time.unwrap_or(0)),
                last_trade_timestamp: order_data.update_time,
                last_update_timestamp: None,
                status: order_data.status
                    .as_ref()
                    .map(|s| self.parse_order_status(s))
                    .unwrap_or(OrderStatus::Open),
                symbol: sym,
                order_type: order_data.order_type
                    .as_ref()
                    .map(|t| self.parse_order_type(t))
                    .unwrap_or(OrderType::Limit),
                time_in_force: None,
                side: order_data.side
                    .as_ref()
                    .map(|s| self.parse_order_side(s))
                    .unwrap_or(OrderSide::Buy),
                price: order_data.price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                average: order_data.avg_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                amount,
                filled,
                remaining: Some(amount - filled),
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                cost: Some(cost),
                trades: vec![],
                reduce_only: None,
                post_only: None,
                fee: None,
                fees: vec![],
                info: serde_json::to_value(&order_data).unwrap_or_default(),
            });
        }

        Ok(orders)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".to_string(), self.get_market_id(s)?);
        }

        let response: Vec<CoinsphOrder> = self.private_request("GET", "/openapi/v1/openOrders", Some(params)).await?;

        let mut orders = Vec::new();
        for order_data in response {
            let sym = self.get_symbol(&order_data.symbol);
            let amount = order_data.orig_qty
                .as_ref()
                .and_then(|q| Decimal::from_str(q).ok())
                .unwrap_or(Decimal::ZERO);

            let filled = order_data.executed_qty
                .as_ref()
                .and_then(|q| Decimal::from_str(q).ok())
                .unwrap_or(Decimal::ZERO);

            let cost = order_data.cummulative_quote_qty
                .as_ref()
                .and_then(|c| Decimal::from_str(c).ok())
                .unwrap_or(Decimal::ZERO);

            orders.push(Order {
                id: order_data.order_id.to_string(),
                client_order_id: order_data.client_order_id.clone(),
                datetime: Some(order_data.time
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()),
                timestamp: Some(order_data.time.unwrap_or(0)),
                last_trade_timestamp: order_data.update_time,
                last_update_timestamp: None,
                status: OrderStatus::Open,
                symbol: sym,
                order_type: order_data.order_type
                    .as_ref()
                    .map(|t| self.parse_order_type(t))
                    .unwrap_or(OrderType::Limit),
                time_in_force: None,
                side: order_data.side
                    .as_ref()
                    .map(|s| self.parse_order_side(s))
                    .unwrap_or(OrderSide::Buy),
                price: order_data.price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                average: order_data.avg_price.as_ref().and_then(|p| Decimal::from_str(p).ok()),
                amount,
                filled,
                remaining: Some(amount - filled),
                stop_price: None,
                trigger_price: None,
                take_profit_price: None,
                stop_loss_price: None,
                cost: Some(cost),
                trades: vec![],
                reduce_only: None,
                post_only: None,
                fee: None,
                fees: vec![],
                info: serde_json::to_value(&order_data).unwrap_or_default(),
            });
        }

        Ok(orders)
    }

    async fn fetch_my_trades(&self, symbol: Option<&str>, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".to_string(), self.get_market_id(s)?);
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.min(1000).to_string());
        }

        let response: Vec<CoinsphMyTrade> = self.private_request("GET", "/openapi/v1/myTrades", Some(params)).await?;

        let mut trades = Vec::new();
        for trade_data in response {
            let sym = self.get_symbol(&trade_data.symbol);
            let price = Decimal::from_str(&trade_data.price).unwrap_or(Decimal::ZERO);
            let amount = Decimal::from_str(&trade_data.qty).unwrap_or(Decimal::ZERO);
            let cost = trade_data.quote_qty
                .as_ref()
                .and_then(|q| Decimal::from_str(q).ok())
                .unwrap_or(price * amount);

            let side = if trade_data.is_buyer { "buy" } else { "sell" };
            let _taker_or_maker = if trade_data.is_maker { "maker" } else { "taker" };

            let fee = if let (Some(comm), Some(asset)) = (&trade_data.commission, &trade_data.commission_asset) {
                Some(Fee {
                    currency: Some(asset.clone()),
                    cost: Some(Decimal::from_str(comm).unwrap_or(Decimal::ZERO)),
                    rate: None,
                })
            } else {
                None
            };

            trades.push(Trade {
                id: trade_data.id.to_string(),
                order: Some(trade_data.order_id.to_string()),
                info: serde_json::json!({}),
                timestamp: Some(trade_data.time),
                datetime: Some(chrono::DateTime::from_timestamp_millis(trade_data.time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()),
                symbol: sym,
                trade_type: None,
                taker_or_maker: None,
                side: Some(side.to_string()),
                price,
                amount,
                cost: Some(cost),
                fee: fee.clone(),
                fees: fee.into_iter().collect(),
            });
        }

        Ok(trades)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_exchange() -> Coinsph {
        let config = ExchangeConfig::default();
        Coinsph::new(config).unwrap()
    }

    #[test]
    fn test_coinsph_creation() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.id(), ExchangeId::Coinsph);
        assert_eq!(exchange.name(), "CoinsPH");
    }

    #[test]
    fn test_exchange_info() {
        let exchange = create_test_exchange();
        assert!(exchange.has().spot);
        assert!(!exchange.has().future);
        assert!(!exchange.has().option);
    }

    #[test]
    fn test_urls() {
        let exchange = create_test_exchange();
        let urls = exchange.urls();
        assert!(urls.api.contains_key("public"));
        assert!(urls.doc.contains(&"https://coins-docs.github.io/rest-api".to_string()));
    }

    #[test]
    fn test_timeframes() {
        let exchange = create_test_exchange();
        let timeframes = exchange.timeframes();
        assert!(!timeframes.is_empty());
        assert!(timeframes.contains_key(&Timeframe::Hour1));
        assert!(timeframes.contains_key(&Timeframe::Day1));
    }

    #[test]
    fn test_get_market_id() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.get_market_id("BTC/PHP").unwrap(), "BTCPHP");
        assert_eq!(exchange.get_market_id("ETH/USDT").unwrap(), "ETHUSDT");
    }

    #[test]
    fn test_parse_order_status() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.parse_order_status("NEW"), OrderStatus::Open);
        assert_eq!(exchange.parse_order_status("FILLED"), OrderStatus::Closed);
        assert_eq!(exchange.parse_order_status("CANCELED"), OrderStatus::Canceled);
        assert_eq!(exchange.parse_order_status("EXPIRED"), OrderStatus::Expired);
        assert_eq!(exchange.parse_order_status("REJECTED"), OrderStatus::Rejected);
    }

    #[test]
    fn test_parse_order_side() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.parse_order_side("BUY"), OrderSide::Buy);
        assert_eq!(exchange.parse_order_side("SELL"), OrderSide::Sell);
    }

    #[test]
    fn test_parse_order_type() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.parse_order_type("LIMIT"), OrderType::Limit);
        assert_eq!(exchange.parse_order_type("MARKET"), OrderType::Market);
        assert_eq!(exchange.parse_order_type("STOP_LOSS"), OrderType::StopLoss);
    }

    #[test]
    fn test_features() {
        let exchange = create_test_exchange();
        let features = exchange.has();
        assert!(features.fetch_ticker);
        assert!(features.fetch_tickers);
        assert!(features.fetch_order_book);
        assert!(features.fetch_trades);
        assert!(features.fetch_ohlcv);
        assert!(features.fetch_balance);
        assert!(features.create_order);
        assert!(features.cancel_order);
        assert!(features.fetch_order);
        assert!(features.fetch_orders);
        assert!(features.fetch_open_orders);
        assert!(features.fetch_my_trades);
    }

    #[test]
    fn test_count_decimals() {
        let exchange = create_test_exchange();
        assert_eq!(exchange.count_decimals("0.00001"), 5);
        assert_eq!(exchange.count_decimals("0.1"), 1);
        assert_eq!(exchange.count_decimals("1"), 0);
        assert_eq!(exchange.count_decimals("0.00000001"), 8);
    }
}
