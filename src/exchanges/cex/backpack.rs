//! Backpack Exchange Implementation
//!
//! Solana-based crypto exchange with spot, margin, and perpetual trading

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
    Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// Backpack exchange - Solana-based spot and perpetuals
pub struct Backpack {
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

impl Backpack {
    const BASE_URL: &'static str = "https://api.backpack.exchange";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second
    const VERSION: &'static str = "v1";

    /// Create new Backpack instance
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
            fetch_order: false,
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
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/cc04c278-679f-4554-9f72-930dd632b80f".into()),
            api: api_urls,
            www: Some("https://backpack.exchange/".into()),
            doc: vec!["https://docs.backpack.exchange/".into()],
            fees: None,
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
        timeframes.insert(Timeframe::Day3, "3d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
        timeframes.insert(Timeframe::Month1, "1month".into());

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

    /// Convert unified symbol to Backpack format (BTC/USDC -> BTC_USDC)
    fn format_symbol(symbol: &str) -> String {
        symbol.replace('/', "_")
    }

    /// Convert Backpack symbol to unified format (BTC_USDC -> BTC/USDC)
    fn to_unified_symbol(backpack_symbol: &str) -> String {
        // Handle perp markets (BTC_USDC_PERP -> BTC/USDC:USDC)
        if backpack_symbol.ends_with("_PERP") {
            let base_symbol = backpack_symbol.trim_end_matches("_PERP");
            let parts: Vec<&str> = base_symbol.split('_').collect();
            if parts.len() >= 2 {
                return format!("{}/{}:{}", parts[0], parts[1], parts[1]);
            }
        }
        // Handle spot markets (BTC_USDC -> BTC/USDC)
        backpack_symbol.replace('_', "/")
    }

    /// Count decimal places in a numeric string (e.g., "0.001" -> 3)
    fn count_decimal_places(s: &str) -> Option<i32> {
        let s = s.trim();
        if let Some(pos) = s.find('.') {
            let decimal_part = &s[pos + 1..];
            // Remove trailing zeros for step sizes like "0.10"
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
            format!("/api/{}/{path}?{query}", Self::VERSION)
        } else {
            format!("/api/{}/{path}", Self::VERSION)
        };

        self.public_client.get(&url, None, None).await
    }

    /// Parse market from API response
    fn parse_market(&self, market: &BackpackMarket) -> Market {
        let market_id = market.symbol.clone();
        let base = market.base_symbol.clone();
        let quote = market.quote_symbol.clone();

        let (market_type, is_perp) = match market.market_type.as_str() {
            "PERP" => (MarketType::Swap, true),
            "SPOT" => (MarketType::Spot, false),
            _ => (MarketType::Spot, false),
        };

        let symbol = if is_perp {
            format!("{base}/{quote}:{quote}")
        } else {
            format!("{base}/{quote}")
        };

        let price_filter = &market.filters.price;
        let quantity_filter = &market.filters.quantity;

        // Calculate precision as number of decimal places
        let price_precision = price_filter.tick_size.as_ref()
            .and_then(|s| Self::count_decimal_places(s))
            .unwrap_or(2);

        let amount_precision = quantity_filter.step_size.as_ref()
            .and_then(|s| Self::count_decimal_places(s))
            .unwrap_or(2);

        Market {
            id: market_id.clone(),
            lowercase_id: Some(market_id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base.clone(),
            quote_id: quote.clone(),
            market_type,
            spot: !is_perp,
            margin: false,
            swap: is_perp,
            future: false,
            option: false,
            index: false,
            active: market.order_book_state == "Open",
            contract: is_perp,
            linear: Some(is_perp),
            inverse: Some(false),
            sub_type: None,
            taker: None,
            maker: None,
            contract_size: if is_perp { Some(Decimal::ONE) } else { None },
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            settle: if is_perp { Some(quote.clone()) } else { None },
            settle_id: if is_perp { Some(quote) } else { None },
            precision: MarketPrecision {
                amount: Some(amount_precision),
                price: Some(price_precision),
                cost: None,
                base: None,
                quote: None,
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: quantity_filter.min_quantity.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                    max: quantity_filter.max_quantity.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                },
                price: MinMax {
                    min: price_filter.min_price.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                    max: price_filter.max_price.as_ref()
                        .and_then(|s| Decimal::from_str(s).ok()),
                },
                cost: MinMax { min: None, max: None },
                leverage: MinMax { min: None, max: None },
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(market).unwrap_or_default(),
            tier_based: false,
            percentage: false,
        }
    }

    /// Parse ticker from API response
    fn parse_ticker(&self, data: &BackpackTicker) -> Ticker {
        let symbol = Self::to_unified_symbol(&data.symbol);
        let timestamp = Utc::now().timestamp_millis();

        Ticker {
            symbol: symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: data.high_price_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            low: data.low_price_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid: data.best_bid.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            bid_volume: data.bid_quantity.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            ask: data.best_ask.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            ask_volume: data.ask_quantity.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            vwap: None,
            open: data.open_price_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            close: data.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            last: data.last_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            previous_close: None,
            change: data.price_change_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            percentage: data.price_change_percent_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            quote_volume: data.quote_volume_24h.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            index_price: None,
            mark_price: data.mark_price.as_ref().and_then(|s| Decimal::from_str(s).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order book from API response
    fn parse_order_book(&self, data: &BackpackOrderBook, symbol: &str) -> OrderBook {
        let timestamp = Utc::now().timestamp_millis();

        let bids: Vec<OrderBookEntry> = data.bids.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&entry[0]).ok()?,
                        amount: Decimal::from_str(&entry[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data.asks.iter()
            .filter_map(|entry| {
                if entry.len() >= 2 {
                    Some(OrderBookEntry {
                        price: Decimal::from_str(&entry[0]).ok()?,
                        amount: Decimal::from_str(&entry[1]).ok()?,
                    })
                } else {
                    None
                }
            })
            .collect();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: data.last_update_id.as_ref().and_then(|s| s.parse().ok()),
            bids,
            asks,
        }
    }

    /// Parse trade from API response
    fn parse_trade(&self, data: &BackpackTrade, symbol: &str) -> Trade {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price = data.price.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();
        let amount = data.quantity.as_ref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or_default();

        Trade {
            id: data.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: chrono::DateTime::from_timestamp_millis(timestamp)
                .map(|dt| dt.to_rfc3339()),
            symbol: symbol.to_string(),
            trade_type: None,
            side: if data.is_buyer_maker.unwrap_or(false) {
                Some("sell".to_string())
            } else {
                Some("buy".to_string())
            },
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: vec![],
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse OHLCV from API response
    fn parse_ohlcv(&self, data: &BackpackKline) -> OHLCV {
        OHLCV {
            timestamp: data.start_time.unwrap_or(0),
            open: data.open.as_ref().and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default(),
            high: data.high.as_ref().and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default(),
            low: data.low.as_ref().and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default(),
            close: data.close.as_ref().and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default(),
            volume: data.volume.as_ref().and_then(|s| Decimal::from_str(s).ok()).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Backpack {
    fn id(&self) -> ExchangeId {
        ExchangeId::Backpack
    }

    fn name(&self) -> &str {
        "Backpack"
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

        let fetched = self.fetch_markets().await?;
        let mut markets_map = HashMap::new();
        for market in fetched {
            markets_map.insert(market.symbol.clone(), market);
        }
        Ok(markets_map)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: Vec<BackpackMarket> = self.public_get("markets", None).await?;

        let markets: Vec<Market> = response.iter()
            .map(|m| self.parse_market(m))
            .collect();

        // Update internal caches
        let mut markets_cache = self.markets.write().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to acquire write lock".to_string(),
        })?;
        let mut markets_by_id_cache = self.markets_by_id.write().map_err(|_| CcxtError::ExchangeError {
            message: "Failed to acquire write lock".to_string(),
        })?;

        for market in &markets {
            markets_cache.insert(market.symbol.clone(), market.clone());
            markets_by_id_cache.insert(market.id.clone(), market.symbol.clone());
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = Self::format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: BackpackTicker = self.public_get("ticker", Some(params)).await?;
        Ok(self.parse_ticker(&response))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<BackpackTicker> = self.public_get("tickers", None).await?;

        let mut tickers = HashMap::new();
        for t in response.iter() {
            let ticker = self.parse_ticker(t);
            if let Some(syms) = symbols {
                if syms.contains(&ticker.symbol.as_str()) {
                    tickers.insert(ticker.symbol.clone(), ticker);
                }
            } else {
                tickers.insert(ticker.symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = Self::format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BackpackOrderBook = self.public_get("depth", Some(params)).await?;
        Ok(self.parse_order_book(&response, symbol))
    }

    async fn fetch_trades(&self, symbol: &str, since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = Self::format_symbol(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BackpackTrade> = self.public_get("trades", Some(params)).await?;

        let mut trades: Vec<Trade> = response.iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        if let Some(since_ts) = since {
            trades.retain(|t| t.timestamp.map(|ts| ts >= since_ts).unwrap_or(true));
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
        let market_id = Self::format_symbol(symbol);
        let interval = self.timeframes.get(&timeframe)
            .cloned()
            .unwrap_or_else(|| "1h".to_string());

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval);
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<BackpackKline> = self.public_get("klines", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response.iter()
            .map(|k| self.parse_ohlcv(k))
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        Err(CcxtError::AuthenticationError {
            message: "Private endpoint requires authentication".into(),
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
            message: "Private endpoint requires authentication".into(),
        })
    }

    async fn cancel_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::AuthenticationError {
            message: "Private endpoint requires authentication".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::AuthenticationError {
            message: "Private endpoint requires authentication".into(),
        })
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::NotSupported {
            feature: "fetchOrder".into(),
        })
    }

    async fn fetch_my_trades(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        Err(CcxtError::AuthenticationError {
            message: "Private endpoint requires authentication".into(),
        })
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(Self::format_symbol(symbol))
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
        // Public API only - no signing needed
        let url = format!("{}{}", Self::BASE_URL, path);
        SignedRequest {
            url,
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

// API Response Types
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackMarket {
    symbol: String,
    base_symbol: String,
    quote_symbol: String,
    market_type: String,
    order_book_state: String,
    filters: BackpackFilters,
    #[serde(default)]
    funding_interval: Option<i64>,
    #[serde(default)]
    funding_rate_lower_bound: Option<String>,
    #[serde(default)]
    funding_rate_upper_bound: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BackpackFilters {
    price: BackpackPriceFilter,
    quantity: BackpackQuantityFilter,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackPriceFilter {
    tick_size: Option<String>,
    min_price: Option<String>,
    max_price: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackQuantityFilter {
    step_size: Option<String>,
    min_quantity: Option<String>,
    max_quantity: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackTicker {
    symbol: String,
    #[serde(default)]
    first_price: Option<String>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    price_change_24h: Option<String>,
    #[serde(default)]
    price_change_percent_24h: Option<String>,
    #[serde(default)]
    high_price_24h: Option<String>,
    #[serde(default)]
    low_price_24h: Option<String>,
    #[serde(default)]
    open_price_24h: Option<String>,
    #[serde(default)]
    volume_24h: Option<String>,
    #[serde(default)]
    quote_volume_24h: Option<String>,
    #[serde(default)]
    trades_24h: Option<i64>,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    bid_quantity: Option<String>,
    #[serde(default)]
    ask_quantity: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    index_price: Option<String>,
    #[serde(default)]
    funding_rate: Option<String>,
    #[serde(default)]
    next_funding_time: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackOrderBook {
    #[serde(default)]
    last_update_id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackTrade {
    id: Option<i64>,
    price: Option<String>,
    quantity: Option<String>,
    quote_quantity: Option<String>,
    time: Option<i64>,
    is_buyer_maker: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackpackKline {
    start_time: Option<i64>,
    open: Option<String>,
    high: Option<String>,
    low: Option<String>,
    close: Option<String>,
    volume: Option<String>,
    #[serde(default)]
    end_time: Option<i64>,
    #[serde(default)]
    trades: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_symbol() {
        assert_eq!(Backpack::format_symbol("BTC/USDC"), "BTC_USDC");
        assert_eq!(Backpack::format_symbol("ETH/USDC"), "ETH_USDC");
    }

    #[test]
    fn test_to_unified_symbol() {
        assert_eq!(Backpack::to_unified_symbol("BTC_USDC"), "BTC/USDC");
        assert_eq!(Backpack::to_unified_symbol("ETH_USDC_PERP"), "ETH/USDC:USDC");
        assert_eq!(Backpack::to_unified_symbol("SOL_USDC"), "SOL/USDC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Backpack::new(config).unwrap();
        assert_eq!(exchange.name(), "Backpack");
        assert!(exchange.has().spot);
        assert!(exchange.has().swap);
    }
}
