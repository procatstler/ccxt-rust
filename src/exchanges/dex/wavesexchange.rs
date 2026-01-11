//! Wavesexchange Exchange Implementation
//!
//! CCXT wavesexchange.ts를 Rust로 포팅

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
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade,
    Transaction, OHLCV,
};

/// Wavesexchange 거래소 (DEX)
pub struct Wavesexchange {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    matcher_client: HttpClient,
    node_client: HttpClient,
    market_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    access_token: RwLock<Option<String>>,
    matcher_public_key: RwLock<Option<String>>,
    waves_address: RwLock<Option<String>>,
}

impl Wavesexchange {
    const MATCHER_URL: &'static str = "https://matcher.wx.network";
    const NODE_URL: &'static str = "https://nodes.wx.network";
    const PUBLIC_URL: &'static str = "https://api.wavesplatform.com/v0";
    const PRIVATE_URL: &'static str = "https://api.wx.network/v1";
    const MARKET_URL: &'static str = "https://wx.network/api/v1/forward/marketdata/api/v1";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 Wavesexchange 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::PUBLIC_URL, &config)?;
        let private_client = HttpClient::new(Self::PRIVATE_URL, &config)?;
        let matcher_client = HttpClient::new(Self::MATCHER_URL, &config)?;
        let node_client = HttpClient::new(Self::NODE_URL, &config)?;
        let market_client = HttpClient::new(Self::MARKET_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
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
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: false,
            fetch_withdrawals: false,
            withdraw: true,
            fetch_deposit_address: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("matcher".into(), Self::MATCHER_URL.into());
        api_urls.insert("node".into(), Self::NODE_URL.into());
        api_urls.insert("public".into(), Self::PUBLIC_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());
        api_urls.insert("market".into(), Self::MARKET_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/84547058-5fb27d80-ad0b-11ea-8711-78ac8b3c7f31.jpg".into()),
            api: api_urls,
            www: Some("https://wx.network".into()),
            doc: vec![
                "https://docs.wx.network".into(),
                "https://docs.waves.tech".into(),
                "https://api.wavesplatform.com/v0/docs/".into(),
            ],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour3, "3h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour12, "12h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

        Ok(Self {
            config,
            public_client,
            private_client,
            matcher_client,
            node_client,
            market_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            access_token: RwLock::new(None),
            matcher_public_key: RwLock::new(None),
            waves_address: RwLock::new(None),
        })
    }

    /// Market API 호출
    async fn market_get<T: serde::de::DeserializeOwned>(
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

        self.market_client.get(&url, None, None).await
    }

    /// Public API 호출
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

    /// Matcher API 호출
    async fn matcher_get<T: serde::de::DeserializeOwned>(
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

        self.matcher_client.get(&url, None, None).await
    }

    /// Private API 호출 (requires auth)
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let access_token = self.access_token.read().unwrap().clone();
        let token = access_token.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Access token required".into(),
        })?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {token}"));

        let url = if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => self.private_client.post(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 ID 파싱 (baseId/quoteId)
    fn to_market_id(&self, symbol: &str) -> CcxtResult<String> {
        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.to_string(),
        })?;
        Ok(market.id.clone())
    }

    /// 티커 파싱
    fn parse_ticker(&self, ticker: &WavesTicker, market: &Market) -> Ticker {
        let timestamp = ticker.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: market.symbol.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker.h24_high,
            low: ticker.h24_low,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: ticker.h24_vwap,
            open: ticker.h24_open,
            close: ticker.h24_close,
            last: ticker.h24_close,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: ticker.h24_volume,
            quote_volume: ticker.h24_price_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, order: &WavesOrder, symbol: &str) -> Order {
        let status = match order.status.as_str() {
            "Accepted" => OrderStatus::Open,
            "PartiallyFilled" => OrderStatus::Open,
            "Filled" => OrderStatus::Closed,
            "Cancelled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match order.order_type.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match order.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let amount: Decimal = order.amount.parse().unwrap_or_default();
        let filled: Decimal = order.filled.parse().unwrap_or_default();
        let price: Option<Decimal> = order.price.as_ref().and_then(|p| p.parse().ok());

        Order {
            id: order.id.clone(),
            client_order_id: None,
            timestamp: Some(order.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(order.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining: Some(amount - filled),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: price.map(|p| p * filled),
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(order).unwrap_or_default(),
        }
    }

    /// 잔고 파싱
    fn parse_balance(&self, balances: &[WavesBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.available.parse().ok();
            let total: Option<Decimal> = b.balance.parse().ok();
            let used = match (total, free) {
                (Some(t), Some(f)) => Some(t - f),
                _ => None,
            };

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(&b.currency, balance);
        }

        result
    }

    /// 거래 파싱
    fn parse_trade(&self, trade: &WavesTrade, symbol: &str) -> Trade {
        let price: Decimal = trade.price.parse().unwrap_or_default();
        let amount: Decimal = trade.amount.parse().unwrap_or_default();

        Trade {
            id: trade.id.clone(),
            order: None,
            timestamp: Some(trade.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(trade.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: trade.side.clone(),
            taker_or_maker: Some(TakerOrMaker::Taker),
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(trade).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Wavesexchange {
    fn id(&self) -> ExchangeId {
        ExchangeId::Wavesexchange
    }

    fn name(&self) -> &str {
        "Waves.Exchange"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["CH"] // Switzerland
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
        let response: Vec<WavesMarket> = self.market_get("/tickers", None).await?;

        let mut markets = Vec::new();

        for entry in response {
            let base_id = entry.amount_asset_id.clone();
            let quote_id = entry.price_asset_id.clone();
            let id = format!("{base_id}/{quote_id}");
            let symbol = entry.symbol.clone();

            let parts: Vec<&str> = symbol.split('/').collect();
            if parts.len() != 2 {
                continue;
            }
            let base = parts[0].to_string();
            let quote = parts[1].to_string();

            let amount_decimals = entry.amount_asset_decimals;
            let price_decimals = entry.price_asset_decimals;

            let market = Market {
                id,
                lowercase_id: None,
                symbol: format!("{base}/{quote}"),
                base: base.clone(),
                quote: quote.clone(),
                base_id: base_id.clone(),
                quote_id: quote_id.clone(),
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
                taker: Some(Decimal::new(3, 3)), // 0.3%
                maker: Some(Decimal::new(3, 3)), // 0.3%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(amount_decimals),
                    price: Some(price_decimals),
                    cost: None,
                    base: Some(amount_decimals),
                    quote: Some(price_decimals),
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&entry).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let market = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).cloned().ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?
        };

        let mut params = HashMap::new();
        params.insert("pairs".into(), market.id.clone());

        let response: WavesPairsResponse = self.public_get("/pairs", Some(params)).await?;

        let ticker = response.data.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data".into(),
        })?;

        Ok(self.parse_ticker(&ticker.data, &market))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let response: Vec<WavesMarket> = self.market_get("/tickers", None).await?;

        let markets = self.markets.read().unwrap();
        let mut tickers = HashMap::new();

        for entry in response {
            let symbol = entry.symbol.clone();

            if let Some(filter) = symbols {
                if !filter.contains(&symbol.as_str()) {
                    continue;
                }
            }

            if let Some(market) = markets.get(&symbol) {
                let ticker_data = WavesTicker {
                    h24_open: entry.h24_open,
                    h24_high: entry.h24_high,
                    h24_low: entry.h24_low,
                    h24_close: entry.h24_close,
                    h24_vwap: entry.h24_vwap,
                    h24_volume: entry.h24_volume,
                    h24_price_volume: entry.h24_price_volume,
                    timestamp: Some(entry.timestamp),
                };
                let ticker = self.parse_ticker(&ticker_data, market);
                tickers.insert(symbol, ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol)?;
        let parts: Vec<&str> = market_id.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("depth".into(), l.to_string());
        }

        let path = format!("/matcher/orderbook/{}/{}", parts[0], parts[1]);
        let response: WavesOrderBook = self.matcher_get(&path, Some(params)).await?;

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price.parse().unwrap_or_default(),
                amount: b.amount.parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price.parse().unwrap_or_default(),
                amount: a.amount.parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(response.timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(response.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
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
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol)?;

        let mut params = HashMap::new();
        params.insert("amountAsset".into(), market_id.split('/').next().unwrap_or("").to_string());
        params.insert("priceAsset".into(), market_id.split('/').nth(1).unwrap_or("").to_string());

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<WavesTrade> = self.public_get("/transactions/exchange", Some(params)).await?;

        let trades: Vec<Trade> = response
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
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol)?;
        let parts: Vec<&str> = market_id.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            });
        }

        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("timeframe".into(), interval.clone());

        if let Some(s) = since {
            params.insert("from".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let path = format!("/candles/{}/{}", parts[0], parts[1]);
        let response: Vec<WavesCandle> = self.public_get(&path, Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .map(|c| OHLCV {
                timestamp: c.time,
                open: c.open.parse().unwrap_or_default(),
                high: c.high.parse().unwrap_or_default(),
                low: c.low.parse().unwrap_or_default(),
                close: c.close.parse().unwrap_or_default(),
                volume: c.volume.parse().unwrap_or_default(),
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let waves_address = self.waves_address.read().unwrap().clone();
        let address = waves_address.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Waves address required".into(),
        })?;

        let path = format!("/addresses/balance/details/{address}");
        let response: WavesBalanceResponse = self.node_client.get(&path, None, None).await?;

        let balances = vec![
            WavesBalance {
                currency: "WAVES".to_string(),
                balance: response.available.to_string(),
                available: response.available.to_string(),
            }
        ];

        Ok(self.parse_balance(&balances))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol)?;

        let order_type_str = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        };

        let side_str = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mut params = HashMap::new();
        params.insert("amountAsset".into(), market_id.split('/').next().unwrap_or("").to_string());
        params.insert("priceAsset".into(), market_id.split('/').nth(1).unwrap_or("").to_string());
        params.insert("orderType".into(), order_type_str.to_string());
        params.insert("side".into(), side_str.to_string());
        params.insert("amount".into(), amount.to_string());

        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let response: WavesOrder = self.private_request("POST", "/matcher/orderbook", params).await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let market_id = self.to_market_id(symbol)?;
        let parts: Vec<&str> = market_id.split('/').collect();

        let path = format!("/matcher/orderbook/{}/{}/cancel", parts[0], parts[1]);

        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let response: WavesOrder = self.private_request("POST", &path, params).await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.load_markets(false).await?;

        let waves_address = self.waves_address.read().unwrap().clone();
        let address = waves_address.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Waves address required".into(),
        })?;

        let path = format!("/matcher/orders/{address}/{id}");
        let response: WavesOrder = self.matcher_get(&path, None).await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let waves_address = self.waves_address.read().unwrap().clone();
        let address = waves_address.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Waves address required".into(),
        })?;

        let mut params = HashMap::new();
        if let Some(s) = symbol {
            let market_id = self.to_market_id(s)?;
            params.insert("amountAsset".into(), market_id.split('/').next().unwrap_or("").to_string());
            params.insert("priceAsset".into(), market_id.split('/').nth(1).unwrap_or("").to_string());
        }

        let path = format!("/matcher/orders/{address}");
        let response: Vec<WavesOrder> = self.matcher_get(&path, Some(params)).await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .iter()
            .filter(|o| o.status == "Accepted" || o.status == "PartiallyFilled")
            .map(|o| {
                let market_id = format!("{}/{}", o.asset_pair.amount_asset, o.asset_pair.price_asset);
                let sym = markets_by_id.get(&market_id).map(|s| s.as_str()).unwrap_or(symbol.unwrap_or(""));
                self.parse_order(o, sym)
            })
            .collect();

        Ok(orders)
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        self.to_market_id(symbol).ok()
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        let markets_by_id = self.markets_by_id.read().unwrap();
        markets_by_id.get(market_id).cloned()
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let base_url = match api {
            "matcher" => Self::MATCHER_URL,
            "node" => Self::NODE_URL,
            "public" => Self::PUBLIC_URL,
            "private" => Self::PRIVATE_URL,
            "market" => Self::MARKET_URL,
            _ => Self::PUBLIC_URL,
        };

        let mut url = format!("{base_url}{path}");
        let mut headers = HashMap::new();

        if api == "private" {
            if let Some(token) = self.access_token.read().unwrap().as_ref() {
                headers.insert("Authorization".into(), format!("Bearer {token}"));
            }
        }

        if !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let waves_address = self.waves_address.read().unwrap().clone();
        let address = waves_address.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Waves address required".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("sender".into(), address.clone());

        if let Some(s) = symbol {
            let market_id = self.to_market_id(s)?;
            params.insert("amountAsset".into(), market_id.split('/').next().unwrap_or("").to_string());
            params.insert("priceAsset".into(), market_id.split('/').nth(1).unwrap_or("").to_string());
        }

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<WavesTrade> = self.public_get("/transactions/exchange", Some(params)).await?;

        let sym = symbol.unwrap_or("");
        let trades: Vec<Trade> = response
            .iter()
            .map(|t| self.parse_trade(t, sym))
            .collect();

        Ok(trades)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let waves_address = self.waves_address.read().unwrap().clone();
        let address = waves_address.as_ref().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Waves address required".into(),
        })?;

        let mut params = HashMap::new();
        if let Some(s) = symbol {
            let market_id = self.to_market_id(s)?;
            params.insert("amountAsset".into(), market_id.split('/').next().unwrap_or("").to_string());
            params.insert("priceAsset".into(), market_id.split('/').nth(1).unwrap_or("").to_string());
        }

        let path = format!("/matcher/orders/{address}");
        let response: Vec<WavesOrder> = self.matcher_get(&path, Some(params)).await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .iter()
            .filter(|o| o.status == "Filled" || o.status == "Cancelled")
            .map(|o| {
                let market_id = format!("{}/{}", o.asset_pair.amount_asset, o.asset_pair.price_asset);
                let sym = markets_by_id.get(&market_id).map(|s| s.as_str()).unwrap_or(symbol.unwrap_or(""));
                self.parse_order(o, sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let path = format!("/deposit/addresses/{code}");
        let response: WavesDepositAddress = self.private_request("GET", &path, HashMap::new()).await?;

        let mut address = DepositAddress::new(response.currency, response.address);

        if let Some(tag) = response.tag {
            address = address.with_tag(tag);
        }

        if let Some(platform) = response.platform {
            address = address.with_network(platform);
        }

        Ok(address)
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        _network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("tag".into(), t.to_string());
        }

        let _response: serde_json::Value = self.private_request("POST", "/withdraw", params).await?;

        Ok(Transaction {
            id: String::new(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            updated: None,
            tx_type: crate::types::TransactionType::Withdrawal,
            currency: code.to_string(),
            network: None,
            amount,
            status: crate::types::TransactionStatus::Pending,
            address: Some(address.to_string()),
            tag: tag.map(String::from),
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::Value::Null,
        })
    }
}

// === Wavesexchange API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WavesMarket {
    symbol: String,
    amount_asset_id: String,
    #[serde(rename = "amountAssetName")]
    _amount_asset_name: String,
    amount_asset_decimals: i32,
    price_asset_id: String,
    #[serde(rename = "priceAssetName")]
    _price_asset_name: String,
    price_asset_decimals: i32,
    #[serde(rename = "24h_open")]
    h24_open: Option<Decimal>,
    #[serde(rename = "24h_high")]
    h24_high: Option<Decimal>,
    #[serde(rename = "24h_low")]
    h24_low: Option<Decimal>,
    #[serde(rename = "24h_close")]
    h24_close: Option<Decimal>,
    #[serde(rename = "24h_vwap")]
    h24_vwap: Option<Decimal>,
    #[serde(rename = "24h_volume")]
    h24_volume: Option<Decimal>,
    #[serde(rename = "24h_priceVolume")]
    h24_price_volume: Option<Decimal>,
    timestamp: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct WavesTicker {
    #[serde(rename = "24h_open")]
    h24_open: Option<Decimal>,
    #[serde(rename = "24h_high")]
    h24_high: Option<Decimal>,
    #[serde(rename = "24h_low")]
    h24_low: Option<Decimal>,
    #[serde(rename = "24h_close")]
    h24_close: Option<Decimal>,
    #[serde(rename = "24h_vwap")]
    h24_vwap: Option<Decimal>,
    #[serde(rename = "24h_volume")]
    h24_volume: Option<Decimal>,
    #[serde(rename = "24h_priceVolume")]
    h24_price_volume: Option<Decimal>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WavesPairsResponse {
    data: Vec<WavesPairData>,
}

#[derive(Debug, Deserialize)]
struct WavesPairData {
    data: WavesTicker,
}

#[derive(Debug, Deserialize)]
struct WavesOrderBook {
    timestamp: i64,
    bids: Vec<WavesOrderBookLevel>,
    asks: Vec<WavesOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
struct WavesOrderBookLevel {
    amount: String,
    price: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WavesOrder {
    id: String,
    #[serde(rename = "type")]
    order_type: String,
    #[serde(rename = "orderType")]
    _order_type_alt: Option<String>,
    side: String,
    amount: String,
    price: Option<String>,
    filled: String,
    status: String,
    timestamp: i64,
    asset_pair: WavesAssetPair,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WavesAssetPair {
    amount_asset: String,
    price_asset: String,
}

#[derive(Debug, Deserialize)]
struct WavesBalance {
    currency: String,
    balance: String,
    available: String,
}

#[derive(Debug, Deserialize)]
struct WavesBalanceResponse {
    available: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct WavesTrade {
    id: String,
    price: String,
    amount: String,
    timestamp: i64,
    side: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WavesCandle {
    time: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

#[derive(Debug, Deserialize)]
struct WavesDepositAddress {
    currency: String,
    address: String,
    tag: Option<String>,
    platform: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Wavesexchange::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Wavesexchange);
        assert_eq!(exchange.name(), "Waves.Exchange");
        assert!(exchange.has().spot);
        assert!(!exchange.has().margin);
        assert!(!exchange.has().swap);
    }
}
