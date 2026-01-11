//! dYdX Exchange Implementation
//!
//! Decentralized cryptocurrency derivatives exchange

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderType,
    SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// dYdX 거래소
pub struct Dydx {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Dydx {
    const BASE_URL: &'static str = "https://api.dydx.exchange/v3";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// 새 dYdX 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: false,
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
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/202864757-9e3ce1d6-a1e1-4e45-a96b-28c3cbf25f62.jpg".into()),
            api: api_urls,
            www: Some("https://dydx.exchange".into()),
            doc: vec![
                "https://docs.dydx.exchange/".into(),
                "https://dydxprotocol.github.io/v3-teacher/".into(),
            ],
            fees: Some("https://dydx.exchange/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1MIN".into());
        timeframes.insert(Timeframe::Minute5, "5MINS".into());
        timeframes.insert(Timeframe::Minute15, "15MINS".into());
        timeframes.insert(Timeframe::Minute30, "30MINS".into());
        timeframes.insert(Timeframe::Hour1, "1HOUR".into());
        timeframes.insert(Timeframe::Hour4, "4HOURS".into());
        timeframes.insert(Timeframe::Day1, "1DAY".into());

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

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, None, None).await
    }

    /// 마켓 ID 변환 (BTC/USD -> BTC-USD)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }
}

#[async_trait]
impl Exchange for Dydx {
    fn id(&self) -> ExchangeId {
        ExchangeId::Dydx
    }

    fn name(&self) -> &str {
        "dYdX"
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
        let response: DydxMarketsResponse = self.public_get("/markets").await?;

        let mut markets = Vec::new();
        for (market_id, info) in response.markets {
            let base = info.base_asset.clone();
            let quote = info.quote_asset.clone();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base.clone(),
                quote_id: quote.clone(),
                market_type: MarketType::Swap,
                spot: false,
                margin: false,
                swap: true,
                future: false,
                option: false,
                index: false,
                active: info.status == "ONLINE",
                contract: true,
                linear: Some(true),
                inverse: Some(false),
                sub_type: Some("linear".to_string()),
                taker: info.taker_fee.as_ref().and_then(|s| s.parse().ok()),
                maker: info.maker_fee.as_ref().and_then(|s| s.parse().ok()),
                contract_size: Some(Decimal::ONE),
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: Some(quote.clone()),
                settle_id: Some(quote.clone()),
                precision: MarketPrecision {
                    amount: info.step_size.as_ref().and_then(|s| {
                        let d: Decimal = s.parse().ok()?;
                        let str_val = d.to_string();
                        if let Some(pos) = str_val.find('.') {
                            Some((str_val.len() - pos - 1) as i32)
                        } else {
                            Some(0)
                        }
                    }),
                    price: info.tick_size.as_ref().and_then(|s| {
                        let d: Decimal = s.parse().ok()?;
                        let str_val = d.to_string();
                        if let Some(pos) = str_val.find('.') {
                            Some((str_val.len() - pos - 1) as i32)
                        } else {
                            Some(0)
                        }
                    }),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };
            markets.push(market);
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
        let path = format!("/markets/{market_id}");
        let response: DydxTickerResponse = self.public_get(&path).await?;

        let market_data = response.market;
        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: market_data
                .oracle_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            low: market_data
                .oracle_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            bid: market_data
                .index_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: market_data
                .index_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: market_data
                .oracle_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            last: market_data
                .oracle_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: market_data.volume_24h.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: market_data
                .index_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            mark_price: market_data
                .oracle_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            info: serde_json::to_value(&market_data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/orderbook/{market_id}");
        let response: DydxOrderBookResponse = self.public_get(&path).await?;

        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<DydxOrderBookLevel>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                let price = e.price.as_ref()?.parse().ok()?;
                let amount = e.size.as_ref()?.parse().ok()?;
                Some(OrderBookEntry { price, amount })
            });
            if let Some(l) = limit {
                iter.take(l as usize).collect()
            } else {
                iter.collect()
            }
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            checksum: None,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/trades/{market_id}");
        let response: DydxTradesResponse = self.public_get(&path).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response
            .trades
            .iter()
            .take(limit)
            .filter_map(|t| {
                let timestamp = t
                    .created_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let price: Decimal = t.price.as_ref()?.parse().ok()?;
                let amount: Decimal = t.size.as_ref()?.parse().ok()?;

                Some(Trade {
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
                    side: t.side.clone(),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(price * amount),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                })
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.convert_to_market_id(symbol);
        let resolution =
            self.timeframes
                .get(&timeframe)
                .ok_or_else(|| CcxtError::NotSupported {
                    feature: format!("Timeframe {timeframe:?}"),
                })?;

        let path = format!("/candles/{market_id}");

        // Build query parameters
        let mut query_params = Vec::new();
        query_params.push(format!("resolution={resolution}"));
        if let Some(l) = limit {
            query_params.push(format!("limit={l}"));
        }

        let full_path = if query_params.is_empty() {
            path
        } else {
            format!("{}?{}", path, query_params.join("&"))
        };

        let response: DydxCandlesResponse = self.public_get(&full_path).await?;

        let ohlcv: Vec<OHLCV> = response
            .candles
            .iter()
            .filter_map(|c| {
                let timestamp = c
                    .started_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())?;

                let open: Decimal = c.open.as_ref()?.parse().ok()?;
                let high: Decimal = c.high.as_ref()?.parse().ok()?;
                let low: Decimal = c.low.as_ref()?.parse().ok()?;
                let close: Decimal = c.close.as_ref()?.parse().ok()?;
                let volume: Decimal = c.base_token_volume.as_ref()?.parse().ok()?;

                Some(OHLCV {
                    timestamp,
                    open,
                    high,
                    low,
                    close,
                    volume,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        Err(CcxtError::AuthenticationError {
            message: "dYdX requires wallet connection for balance queries".into(),
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
            message: "dYdX requires wallet connection for trading".into(),
        })
    }

    async fn cancel_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::AuthenticationError {
            message: "dYdX requires wallet connection for trading".into(),
        })
    }

    async fn fetch_order(&self, _id: &str, _symbol: &str) -> CcxtResult<Order> {
        Err(CcxtError::AuthenticationError {
            message: "dYdX requires wallet connection for order queries".into(),
        })
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        Err(CcxtError::AuthenticationError {
            message: "dYdX requires wallet connection for order queries".into(),
        })
    }
}

// === Response Types ===

#[derive(Debug, Deserialize)]
struct DydxMarketsResponse {
    #[serde(default)]
    markets: HashMap<String, DydxMarketInfo>,
}

#[derive(Debug, Deserialize)]
struct DydxTickerResponse {
    #[serde(default)]
    market: DydxMarketInfo,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DydxMarketInfo {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "baseAsset")]
    base_asset: String,
    #[serde(default, rename = "quoteAsset")]
    quote_asset: String,
    #[serde(default, rename = "stepSize")]
    step_size: Option<String>,
    #[serde(default, rename = "tickSize")]
    tick_size: Option<String>,
    #[serde(default, rename = "indexPrice")]
    index_price: Option<String>,
    #[serde(default, rename = "oraclePrice")]
    oracle_price: Option<String>,
    #[serde(default, rename = "priceChange24H")]
    price_change_24h: Option<String>,
    #[serde(default, rename = "nextFundingRate")]
    next_funding_rate: Option<String>,
    #[serde(default, rename = "nextFundingAt")]
    next_funding_at: Option<String>,
    #[serde(default, rename = "minOrderSize")]
    min_order_size: Option<String>,
    #[serde(default, rename = "type")]
    market_type: Option<String>,
    #[serde(default, rename = "initialMarginFraction")]
    initial_margin_fraction: Option<String>,
    #[serde(default, rename = "maintenanceMarginFraction")]
    maintenance_margin_fraction: Option<String>,
    #[serde(default, rename = "volume24H")]
    volume_24h: Option<String>,
    #[serde(default, rename = "trades24H")]
    trades_24h: Option<String>,
    #[serde(default, rename = "openInterest")]
    open_interest: Option<String>,
    #[serde(default, rename = "incrementalInitialMarginFraction")]
    incremental_initial_margin_fraction: Option<String>,
    #[serde(default, rename = "incrementalPositionSize")]
    incremental_position_size: Option<String>,
    #[serde(default, rename = "maxPositionSize")]
    max_position_size: Option<String>,
    #[serde(default, rename = "baselinePositionSize")]
    baseline_position_size: Option<String>,
    #[serde(default, rename = "assetResolution")]
    asset_resolution: Option<String>,
    #[serde(default, rename = "syntheticAssetId")]
    synthetic_asset_id: Option<String>,
    #[serde(default)]
    taker_fee: Option<String>,
    #[serde(default)]
    maker_fee: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxOrderBookResponse {
    #[serde(default)]
    bids: Vec<DydxOrderBookLevel>,
    #[serde(default)]
    asks: Vec<DydxOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct DydxOrderBookLevel {
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxTradesResponse {
    #[serde(default)]
    trades: Vec<DydxTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DydxTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DydxCandlesResponse {
    #[serde(default)]
    candles: Vec<DydxCandle>,
}

#[derive(Debug, Deserialize)]
struct DydxCandle {
    #[serde(default, rename = "startedAt")]
    started_at: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default, rename = "baseTokenVolume")]
    base_token_volume: Option<String>,
    #[serde(default, rename = "usdVolume")]
    usd_volume: Option<String>,
    #[serde(default)]
    trades: Option<i64>,
    #[serde(default, rename = "startingOpenInterest")]
    starting_open_interest: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Dydx::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Dydx);
        assert_eq!(exchange.name(), "dYdX");
        assert!(exchange.has().swap);
        assert!(!exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Dydx::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USD"), "BTC-USD");
    }

    #[test]
    fn test_timeframes() {
        let config = ExchangeConfig::new();
        let exchange = Dydx::new(config).unwrap();
        assert_eq!(
            exchange.timeframes().get(&Timeframe::Minute1),
            Some(&"1MIN".to_string())
        );
        assert_eq!(
            exchange.timeframes().get(&Timeframe::Hour1),
            Some(&"1HOUR".to_string())
        );
        assert_eq!(
            exchange.timeframes().get(&Timeframe::Day1),
            Some(&"1DAY".to_string())
        );
    }
}
