//! Toobit Exchange Implementation
//!
//! Cayman Islands-based crypto exchange with spot and perpetual trading

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, MarginModes, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// Toobit exchange - Spot and perpetuals trading
pub struct Toobit {
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

impl Toobit {
    const BASE_URL: &'static str = "https://api.toobit.com";
    const RATE_LIMIT_MS: u64 = 20; // 50 requests per second
    const VERSION: &'static str = "v1";

    /// Create new Toobit instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            ws: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("common".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/0c7a97d5-182c-492e-b921-23540c868e0e"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://www.toobit.com/".into()),
            doc: vec![
                "https://toobit-docs.github.io/apidocs/spot/v1/en/".into(),
                "https://toobit-docs.github.io/apidocs/usdt_swap/v1/en/".into(),
            ],
            fees: Some("https://www.toobit.com/fee".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour8, "8h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

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

    /// Convert unified symbol to exchange format
    fn format_symbol(&self, symbol: &str) -> String {
        // BTC/USDT -> BTCUSDT
        symbol.replace('/', "")
    }

    /// Convert exchange symbol to unified format
    fn to_unified_symbol(&self, market_id: &str) -> String {
        // Try to find in markets_by_id first
        if let Ok(markets) = self.markets_by_id.read() {
            if let Some(symbol) = markets.get(market_id) {
                return symbol.clone();
            }
        }
        // Fallback: try to parse symbol
        market_id.to_string()
    }

    /// Get precision from string like "0.01" -> 2
    fn precision_from_string(s: &str) -> Option<i32> {
        if let Some(pos) = s.find('.') {
            let decimal_part = &s[pos + 1..];
            let trimmed = decimal_part.trim_end_matches('0');
            Some(trimmed.len() as i32)
        } else {
            Some(0)
        }
    }

    /// Public API call
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
            format!("/{path}?{query}")
        } else {
            format!("/{path}")
        };

        self.public_client.get(&url, None, None).await
    }

    /// Parse market from API response
    fn parse_market(&self, market: &ToobitMarket, is_contract: bool) -> Market {
        let market_id = market.symbol.clone();
        let base = market.base_asset.clone();
        let quote = market.quote_asset.clone();

        let (market_type, is_swap) = if is_contract {
            (MarketType::Swap, true)
        } else {
            (MarketType::Spot, false)
        };

        let symbol = if is_swap {
            format!("{base}/{quote}:{quote}")
        } else {
            format!("{base}/{quote}")
        };

        // Parse filters
        let mut price_precision = 8;
        let mut amount_precision = 8;
        let mut min_price = Decimal::ZERO;
        let mut max_price = Decimal::MAX;
        let mut min_amount = Decimal::ZERO;
        let mut max_amount = Decimal::MAX;
        let mut min_cost = Decimal::ZERO;
        let mut _tick_size = Decimal::ZERO;
        let mut _step_size = Decimal::ZERO;

        for filter in &market.filters {
            match filter.filter_type.as_str() {
                "PRICE_FILTER" => {
                    if let Some(ref ts) = filter.tick_size {
                        if let Some(prec) = Self::precision_from_string(ts) {
                            price_precision = prec;
                        }
                        _tick_size = Decimal::from_str(ts).unwrap_or(Decimal::ZERO);
                    }
                    if let Some(ref mp) = filter.min_price {
                        min_price = Decimal::from_str(mp).unwrap_or(Decimal::ZERO);
                    }
                    if let Some(ref mp) = filter.max_price {
                        max_price = Decimal::from_str(mp).unwrap_or(Decimal::MAX);
                    }
                },
                "LOT_SIZE" => {
                    if let Some(ref ss) = filter.step_size {
                        if let Some(prec) = Self::precision_from_string(ss) {
                            amount_precision = prec;
                        }
                        _step_size = Decimal::from_str(ss).unwrap_or(Decimal::ZERO);
                    }
                    if let Some(ref mq) = filter.min_qty {
                        min_amount = Decimal::from_str(mq).unwrap_or(Decimal::ZERO);
                    }
                    if let Some(ref mq) = filter.max_qty {
                        max_amount = Decimal::from_str(mq).unwrap_or(Decimal::MAX);
                    }
                },
                "MIN_NOTIONAL" => {
                    if let Some(ref mn) = filter.min_notional {
                        min_cost = Decimal::from_str(mn).unwrap_or(Decimal::ZERO);
                    }
                },
                _ => {},
            }
        }

        let is_active = market.status == "TRADING";

        Market {
            id: market_id.clone(),
            lowercase_id: Some(market_id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            settle: if is_swap { Some(quote.clone()) } else { None },
            base_id: base,
            quote_id: quote.clone(),
            settle_id: if is_swap { Some(quote) } else { None },
            market_type,
            spot: !is_swap,
            margin: market.allow_margin.unwrap_or(false),
            swap: is_swap,
            future: false,
            option: false,
            index: false,
            active: is_active,
            contract: is_swap,
            linear: if is_swap { Some(true) } else { None },
            inverse: if is_swap { Some(false) } else { None },
            taker: Some(Decimal::from_str("0.001").unwrap()),
            maker: Some(Decimal::from_str("0.001").unwrap()),
            contract_size: if is_swap {
                market
                    .contract_multiplier
                    .as_ref()
                    .and_then(|s| Decimal::from_str(s).ok())
            } else {
                None
            },
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            precision: MarketPrecision {
                amount: Some(amount_precision),
                price: Some(price_precision),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                leverage: MinMax {
                    min: None,
                    max: None,
                },
                amount: MinMax {
                    min: Some(min_amount),
                    max: Some(max_amount),
                },
                price: MinMax {
                    min: Some(min_price),
                    max: Some(max_price),
                },
                cost: MinMax {
                    min: Some(min_cost),
                    max: None,
                },
            },
            margin_modes: Some(MarginModes::default()),
            percentage: false,
            tier_based: false,
            sub_type: None,
            created: None,
            info: serde_json::to_value(market).unwrap_or_default(),
        }
    }

    /// Parse ticker from API response
    fn parse_ticker(&self, data: &ToobitTicker, symbol: &str) -> Ticker {
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());
        let last = data.c.as_ref().and_then(|s| Decimal::from_str(s).ok());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            low: data.l.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.o.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            close: last,
            last,
            previous_close: None,
            change: data.pc.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            percentage: data.pcp.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            average: None,
            base_volume: data.v.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: data.qv.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse trade from API response
    fn parse_trade(&self, data: &ToobitTrade, symbol: &str) -> Trade {
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data
            .p
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let amount = data
            .q
            .as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();

        // ibm = is buyer maker, so if true, taker was seller
        let side = if data.ibm.unwrap_or(false) {
            Some("sell".to_string())
        } else {
            Some("buy".to_string())
        };

        Trade {
            id: data.v.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
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

    /// Parse order book entry
    fn parse_order_book_entry(&self, entry: &[String]) -> Option<OrderBookEntry> {
        if entry.len() >= 2 {
            let price = Decimal::from_str(&entry[0]).ok()?;
            let amount = Decimal::from_str(&entry[1]).ok()?;
            Some(OrderBookEntry { price, amount })
        } else {
            None
        }
    }

    /// Parse order book from API response
    fn parse_order_book(&self, data: &ToobitOrderBook, symbol: &str) -> OrderBook {
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .b
            .iter()
            .filter_map(|entry| self.parse_order_book_entry(entry))
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .a
            .iter()
            .filter_map(|entry| self.parse_order_book_entry(entry))
            .collect();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            bids,
            asks,
            checksum: None,
        }
    }

    /// Parse OHLCV from API response
    fn parse_ohlcv(&self, data: &ToobitKline) -> OHLCV {
        OHLCV {
            timestamp: data.t.unwrap_or(0),
            open: data
                .o
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            high: data
                .h
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            low: data
                .l
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            close: data
                .c
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
            volume: data
                .v
                .as_ref()
                .and_then(|s| Decimal::from_str(s).ok())
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Toobit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Toobit
    }

    fn name(&self) -> &str {
        "Toobit"
    }

    fn urls(&self) -> &ExchangeUrls {
        &self.urls
    }

    fn timeframes(&self) -> &HashMap<Timeframe, String> {
        &self.timeframes
    }

    async fn load_markets(&self, _reload: bool) -> CcxtResult<HashMap<String, Market>> {
        // Check if already loaded
        {
            let markets = self.markets.read().map_err(|_| CcxtError::ExchangeError {
                message: "toobit: Failed to acquire read lock".into(),
            })?;
            if !markets.is_empty() {
                return Ok(markets.clone());
            }
        }

        let fetched = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        let mut markets_by_id_map = HashMap::new();

        for market in fetched {
            markets_by_id_map.insert(market.id.clone(), market.symbol.clone());
            markets_map.insert(market.symbol.clone(), market);
        }

        // Store markets
        {
            let mut markets = self.markets.write().map_err(|_| CcxtError::ExchangeError {
                message: "toobit: Failed to acquire write lock".into(),
            })?;
            *markets = markets_map.clone();
        }

        {
            let mut markets_by_id =
                self.markets_by_id
                    .write()
                    .map_err(|_| CcxtError::ExchangeError {
                        message: "toobit: Failed to acquire write lock".into(),
                    })?;
            *markets_by_id = markets_by_id_map;
        }

        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: ToobitExchangeInfo = self.public_get("api/v1/exchangeInfo", None).await?;

        let mut markets = Vec::new();

        // Parse spot markets
        for market_data in &response.symbols {
            let market = self.parse_market(market_data, false);
            markets.push(market);
        }

        // Parse contract markets
        for market_data in &response.contracts {
            let market = self.parse_market(market_data, true);
            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);

        let response: Vec<ToobitTicker> = self
            .public_get("quote/v1/ticker/24hr", Some(params))
            .await?;

        if response.is_empty() {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }

        Ok(self.parse_ticker(&response[0], symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<ToobitTicker> = self.public_get("quote/v1/ticker/24hr", None).await?;

        let mut tickers = HashMap::new();
        for ticker_data in &response {
            if let Some(ref s) = ticker_data.s {
                let symbol = self.to_unified_symbol(s);
                let ticker = self.parse_ticker(ticker_data, &symbol);

                // Filter by requested symbols if provided
                if let Some(syms) = symbols {
                    if syms.contains(&symbol.as_str()) {
                        tickers.insert(symbol, ticker);
                    }
                } else {
                    tickers.insert(symbol, ticker);
                }
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: ToobitOrderBook = self.public_get("quote/v1/depth", Some(params)).await?;
        Ok(self.parse_order_book(&response, symbol))
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<ToobitTrade> = self.public_get("quote/v1/trades", Some(params)).await?;

        Ok(response
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.format_symbol(symbol);
        let tf_str = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("toobit: Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("symbol".to_string(), market_id);
        params.insert("interval".to_string(), tf_str.clone());
        if let Some(s) = since {
            params.insert("startTime".to_string(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".to_string(), l.to_string());
        }

        let response: Vec<ToobitKline> = self.public_get("quote/v1/klines", Some(params)).await?;

        Ok(response.iter().map(|k| self.parse_ohlcv(k)).collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        Err(CcxtError::AuthenticationError {
            message: "toobit: Authentication required for balance".into(),
        })
    }

    async fn create_order(
        &self,
        _symbol: &str,
        _order_type: OrderType,
        _side: OrderSide,
        _amount: Decimal,
        _price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        Err(CcxtError::AuthenticationError {
            message: "toobit: Authentication required for orders".into(),
        })
    }

    async fn cancel_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::AuthenticationError {
            message: "toobit: Authentication required for orders".into(),
        })
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::AuthenticationError {
            message: "toobit: Authentication required for orders".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::AuthenticationError {
            message: "toobit: Authentication required for orders".into(),
        })
    }

    fn has(&self) -> &ExchangeFeatures {
        &self.features
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.format_symbol(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().ok()?;
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        _api: &str,
        _method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}/{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        headers.insert("Content-Type".into(), "application/json".into());

        SignedRequest {
            url,
            method: "GET".into(),
            headers,
            body: None,
        }
    }
}

// API Response types

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToobitExchangeInfo {
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    server_time: Option<String>,
    #[serde(default)]
    symbols: Vec<ToobitMarket>,
    #[serde(default)]
    contracts: Vec<ToobitMarket>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToobitMarket {
    symbol: String,
    #[serde(default)]
    symbol_name: Option<String>,
    status: String,
    base_asset: String,
    #[serde(default)]
    base_asset_name: Option<String>,
    #[serde(default)]
    base_asset_precision: Option<String>,
    quote_asset: String,
    #[serde(default)]
    quote_asset_name: Option<String>,
    #[serde(default)]
    quote_precision: Option<String>,
    #[serde(default)]
    quote_asset_precision: Option<String>,
    #[serde(default)]
    iceberg_allowed: Option<bool>,
    #[serde(default)]
    is_aggregate: Option<bool>,
    #[serde(default)]
    allow_margin: Option<bool>,
    #[serde(default)]
    filters: Vec<ToobitFilter>,
    // Contract-specific fields
    #[serde(default)]
    inverse: Option<bool>,
    #[serde(default)]
    index: Option<String>,
    #[serde(default)]
    index_token: Option<String>,
    #[serde(default)]
    margin_token: Option<String>,
    #[serde(default)]
    margin_precision: Option<String>,
    #[serde(default)]
    contract_multiplier: Option<String>,
    #[serde(default)]
    underlying: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToobitFilter {
    filter_type: String,
    #[serde(default)]
    min_price: Option<String>,
    #[serde(default)]
    max_price: Option<String>,
    #[serde(default)]
    tick_size: Option<String>,
    #[serde(default)]
    min_qty: Option<String>,
    #[serde(default)]
    max_qty: Option<String>,
    #[serde(default)]
    step_size: Option<String>,
    #[serde(default)]
    min_notional: Option<String>,
    #[serde(default)]
    min_amount: Option<String>,
    #[serde(default)]
    max_amount: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToobitTicker {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    s: Option<String>,
    #[serde(default)]
    o: Option<String>,
    #[serde(default)]
    h: Option<String>,
    #[serde(default)]
    l: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    v: Option<String>,
    #[serde(default)]
    qv: Option<String>,
    #[serde(default)]
    pc: Option<String>,
    #[serde(default)]
    pcp: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToobitOrderBook {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    b: Vec<Vec<String>>,
    #[serde(default)]
    a: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToobitTrade {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    p: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    ibm: Option<bool>,
    #[serde(default)]
    v: Option<String>,
    #[serde(default)]
    m: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToobitKline {
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    o: Option<String>,
    #[serde(default)]
    h: Option<String>,
    #[serde(default)]
    l: Option<String>,
    #[serde(default)]
    c: Option<String>,
    #[serde(default)]
    v: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        let config = ExchangeConfig::default();
        let exchange = Toobit::new(config).unwrap();
        assert_eq!(exchange.format_symbol("BTC/USDT"), "BTCUSDT");
        assert_eq!(exchange.format_symbol("ETH/USDT"), "ETHUSDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Toobit::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Toobit);
        assert_eq!(exchange.name(), "Toobit");
    }

    #[test]
    fn test_precision_from_string() {
        assert_eq!(Toobit::precision_from_string("0.01"), Some(2));
        assert_eq!(Toobit::precision_from_string("0.0001"), Some(4));
        assert_eq!(Toobit::precision_from_string("1"), Some(0));
        assert_eq!(Toobit::precision_from_string("0.10"), Some(1));
    }
}
