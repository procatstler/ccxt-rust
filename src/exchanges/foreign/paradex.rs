//! Paradex Exchange Implementation
//!
//! Paradex is a decentralized exchange built on the StarkWare layer 2 scaling solution

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

/// Paradex exchange
pub struct Paradex {
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

impl Paradex {
    const BASE_URL: &'static str = "https://api.prod.paradex.trade/v1";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// Create new Paradex instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: false,
            margin: false,
            swap: true,
            future: false,
            option: false,
            fetch_markets: true,
            fetch_currencies: false,
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
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());
        api_urls.insert("test".into(), "https://api.testnet.paradex.trade/v1".into());

        let urls = ExchangeUrls {
            logo: Some("https://github.com/user-attachments/assets/84628770-784e-4ec4-a759-ec2fbb2244ea".into()),
            api: api_urls,
            www: Some("https://www.paradex.trade/".into()),
            doc: vec!["https://docs.api.testnet.paradex.trade/".into()],
            fees: Some("https://docs.paradex.trade/getting-started/trading-fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute3, "3".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());

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

    /// Public API GET request
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

    /// Private API GET request (requires authentication)
    async fn private_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        // Note: Paradex uses wallet-based authentication with ETH/StarkNet signatures
        // This is a simplified implementation - full auth would require signing logic
        let _api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Wallet address required".into(),
        })?;

        let headers = HashMap::new();
        // TODO: Add proper authentication headers with signatures

        self.private_client.get(path, Some(params), Some(headers)).await
    }

    /// Private API POST request (requires authentication)
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        // Note: Paradex uses wallet-based authentication with ETH/StarkNet signatures
        let _api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Wallet address required".into(),
        })?;

        let headers = HashMap::new();
        // TODO: Add proper authentication headers with signatures

        // Convert params to JSON Value
        let body = if params.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&params).unwrap_or_default())
        };

        self.private_client.post(path, body, Some(headers)).await
    }

    /// Private API DELETE request (requires authentication)
    async fn private_delete<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        // Note: Paradex uses wallet-based authentication with ETH/StarkNet signatures
        let _api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Wallet address required".into(),
        })?;

        let headers = HashMap::new();
        // TODO: Add proper authentication headers with signatures

        self.private_client.delete(path, Some(params), Some(headers)).await
    }

    /// Parse market data
    fn parse_market(&self, data: &ParadexMarket) -> Market {
        let base = data.base_currency.clone();
        let quote = data.quote_currency.clone();
        let settle = data.settlement_currency.clone();
        let symbol = format!("{base}/{quote}:{settle}");

        let is_option = data.asset_kind == "PERP_OPTION";
        let market_type = if is_option { MarketType::Option } else { MarketType::Swap };

        Market {
            id: data.symbol.clone(),
            lowercase_id: Some(data.symbol.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: data.base_currency.clone(),
            quote_id: data.quote_currency.clone(),
            settle: Some(settle.clone()),
            settle_id: Some(data.settlement_currency.clone()),
            active: true,
            market_type,
            spot: false,
            margin: false,
            swap: !is_option,
            future: false,
            option: is_option,
            index: false,
            contract: true,
            linear: Some(true),
            inverse: Some(false),
            sub_type: None,
            taker: Some(Decimal::new(2, 4)), // 0.0002
            maker: Some(Decimal::new(2, 4)), // 0.0002
            contract_size: Some(Decimal::ONE),
            expiry: data.expiry_at.filter(|&e| e > 0),
            expiry_datetime: None,
            strike: data.strike_price.as_ref().and_then(|s| s.parse().ok()),
            option_type: data.option_type.clone().map(|s| s.to_lowercase()),
            precision: MarketPrecision {
                amount: data.order_size_increment.parse().ok(),
                price: data.price_tick_size.parse().ok(),
                cost: None,
                base: data.order_size_increment.parse().ok(),
                quote: data.price_tick_size.parse().ok(),
            },
            limits: MarketLimits {
                leverage: MinMax {
                    min: None,
                    max: None,
                },
                amount: MinMax {
                    min: None,
                    max: data.max_order_size.as_ref().and_then(|s| s.parse().ok()),
                },
                price: MinMax {
                    min: None,
                    max: None,
                },
                cost: MinMax {
                    min: data.min_notional.parse().ok(),
                    max: None,
                },
            },
            margin_modes: None,
            created: data.open_at,
            info: serde_json::to_value(data).unwrap_or_default(),
            tier_based: false,
            percentage: true,
        }
    }

    /// Parse ticker data
    fn parse_ticker(&self, data: &ParadexTicker, symbol: &str) -> Ticker {
        let timestamp = data.created_at;

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: None,
            low: None,
            bid: data.bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last_traded_price.as_ref().and_then(|s| s.parse().ok()),
            last: data.last_traded_price.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: data.price_change_rate_24h.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.volume_24h.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: data.oracle_price.as_ref().and_then(|s| s.parse().ok()),
            mark_price: data.mark_price.as_ref().and_then(|s| s.parse().ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order data
    fn parse_order(&self, data: &ParadexOrder, symbol: &str) -> Order {
        let status = match data.status.as_str() {
            "OPEN" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            "PENDING" => OrderStatus::Open,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.size.parse().unwrap_or_default();
        let filled: Decimal = data.filled_size.parse().unwrap_or_default();
        let remaining = Some(amount - filled);

        Order {
            id: data.id.clone(),
            client_order_id: data.client_id.clone(),
            timestamp: Some(data.created_at),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.created_at)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.updated_at,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: data.trigger_price.as_ref().and_then(|p| p.parse().ok()),
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse trade data
    fn parse_trade(&self, data: &ParadexTrade, symbol: &str) -> Trade {
        let timestamp = data.created_at;
        let price: Decimal = data.price.parse().unwrap_or_default();
        let amount: Decimal = data.size.parse().unwrap_or_default();
        let cost = price * amount;

        Trade {
            id: data.id.to_string(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(data.side.to_lowercase()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(cost),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance data
    fn parse_balance(&self, balances: &[ParadexBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.balance.parse().ok();
            let used: Option<Decimal> = b.in_orders.parse().ok();
            let total = match (free, used) {
                (Some(f), Some(u)) => Some(f + u),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.token, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Paradex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Paradex
    }

    fn name(&self) -> &str {
        "Paradex"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &[]
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
        let response: ParadexMarketsResponse = self
            .public_get("/markets", None)
            .await?;

        let mut markets = Vec::new();
        for market_data in response.results {
            markets.push(self.parse_market(&market_data));
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        let market_id = market.id.clone();

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);

        let response: ParadexSummaryResponse = self
            .public_get("/markets/summary", Some(params))
            .await?;

        let ticker_data = response.results.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data returned".into(),
        })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let mut params = HashMap::new();
        params.insert("market".into(), "ALL".to_string());

        let response: ParadexSummaryResponse = self
            .public_get("/markets/summary", Some(params))
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut tickers = HashMap::new();

        for data in response.results {
            if let Some(symbol) = markets_by_id.get(&data.symbol) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let ticker = self.parse_ticker(&data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        let market_id = market.id.clone();

        let path = format!("/orderbook/{market_id}");
        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ParadexOrderBook = self
            .public_get(&path, if params.is_empty() { None } else { Some(params) })
            .await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b[0].parse().unwrap_or_default(),
                amount: b[1].parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a[0].parse().unwrap_or_default(),
                amount: a[1].parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(response.last_updated_at),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.last_updated_at)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: Some(response.seq_no),
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
        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        let market_id = market.id.clone();

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ParadexTradesResponse = self
            .public_get("/trades", Some(params))
            .await?;

        let trades: Vec<Trade> = response
            .results
            .iter()
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
        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        let market_id = market.id.clone();

        let resolution = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("resolution".into(), resolution.clone());

        let now = Utc::now().timestamp_millis();
        if let Some(s) = since {
            params.insert("start_at".into(), s.to_string());
            params.insert("end_at".into(), now.to_string());
        } else if let Some(l) = limit {
            let duration = match timeframe {
                Timeframe::Minute1 => 60 * 1000,
                Timeframe::Minute3 => 180 * 1000,
                Timeframe::Minute5 => 300 * 1000,
                Timeframe::Minute15 => 900 * 1000,
                Timeframe::Minute30 => 1800 * 1000,
                Timeframe::Hour1 => 3600 * 1000,
                _ => 60 * 1000,
            };
            params.insert("start_at".into(), (now - duration * l as i64).to_string());
            params.insert("end_at".into(), now.to_string());
        }

        let response: ParadexOHLCVResponse = self
            .public_get("/markets/klines", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .results
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].as_i64()?,
                    open: c[1].as_f64()?.try_into().ok()?,
                    high: c[2].as_f64()?.try_into().ok()?,
                    low: c[3].as_f64()?.try_into().ok()?,
                    close: c[4].as_f64()?.try_into().ok()?,
                    volume: c[5].as_f64()?.try_into().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: ParadexBalanceResponse = self
            .private_get("/balance", HashMap::new())
            .await?;

        Ok(self.parse_balance(&response.results))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        let market_id = market.id.clone();

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("side".into(), match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }.into());
        params.insert("type".into(), match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        }.into());
        params.insert("size".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: ParadexOrder = self
            .private_post("/orders", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");

        let response: ParadexOrder = self
            .private_delete(&path, HashMap::new())
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let path = format!("/orders/{id}");

        let response: ParadexOrder = self
            .private_get(&path, HashMap::new())
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap().clone();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.to_string(),
            })?;
            params.insert("market".into(), market.id.clone());
        }

        let response: ParadexOrdersResponse = self
            .private_get("/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .results
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.market)
                    .cloned()
                    .unwrap_or_else(|| o.market.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Paradex cancelAllOrders requires a symbol".into(),
        })?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        let market_id = market.id.clone();

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);

        let response: ParadexOrdersResponse = self
            .private_delete("/orders", params)
            .await?;

        let orders: Vec<Order> = response
            .results
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap().clone();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.to_string(),
            })?;
            params.insert("market".into(), market.id.clone());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ParadexTradesResponse = self
            .private_get("/fills", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let trades: Vec<Trade> = response
            .results
            .iter()
            .map(|t| {
                let sym = markets_by_id
                    .get(&t.market)
                    .cloned()
                    .unwrap_or_else(|| t.market.clone());
                self.parse_trade(t, &sym)
            })
            .collect();

        Ok(trades)
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
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let headers = HashMap::new();

        if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        // Note: Paradex requires wallet signature authentication
        // Full implementation would add Authorization header with signed message

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// === Paradex API Response Types ===

#[derive(Debug, Deserialize)]
struct ParadexMarketsResponse {
    results: Vec<ParadexMarket>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ParadexMarket {
    symbol: String,
    base_currency: String,
    quote_currency: String,
    settlement_currency: String,
    order_size_increment: String,
    price_tick_size: String,
    min_notional: String,
    open_at: Option<i64>,
    expiry_at: Option<i64>,
    asset_kind: String,
    #[serde(default)]
    max_order_size: Option<String>,
    #[serde(default)]
    strike_price: Option<String>,
    #[serde(default)]
    option_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParadexSummaryResponse {
    results: Vec<ParadexTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ParadexTicker {
    symbol: String,
    #[serde(default)]
    oracle_price: Option<String>,
    #[serde(default)]
    mark_price: Option<String>,
    #[serde(default)]
    last_traded_price: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    volume_24h: Option<String>,
    created_at: i64,
    #[serde(default)]
    price_change_rate_24h: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParadexOrderBook {
    last_updated_at: i64,
    seq_no: i64,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ParadexTrade {
    id: i64,
    market: String,
    price: String,
    size: String,
    side: String,
    created_at: i64,
}

#[derive(Debug, Deserialize)]
struct ParadexTradesResponse {
    results: Vec<ParadexTrade>,
}

#[derive(Debug, Deserialize)]
struct ParadexOHLCVResponse {
    results: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ParadexOrder {
    id: String,
    #[serde(default)]
    client_id: Option<String>,
    market: String,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    size: String,
    #[serde(default)]
    price: Option<String>,
    status: String,
    filled_size: String,
    created_at: i64,
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    trigger_price: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParadexOrdersResponse {
    results: Vec<ParadexOrder>,
}

#[derive(Debug, Deserialize)]
struct ParadexBalanceResponse {
    results: Vec<ParadexBalance>,
}

#[derive(Debug, Deserialize)]
struct ParadexBalance {
    token: String,
    balance: String,
    in_orders: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let paradex = Paradex::new(config).unwrap();

        assert_eq!(paradex.id(), ExchangeId::Paradex);
        assert_eq!(paradex.name(), "Paradex");
        assert!(paradex.has().swap);
        assert!(!paradex.has().spot);
    }
}
