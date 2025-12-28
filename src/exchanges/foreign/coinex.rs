//! CoinEx Exchange Implementation
//!
//! CoinEx 거래소 구현체

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// CoinEx 거래소
pub struct Coinex {
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

impl Coinex {
    const BASE_URL: &'static str = "https://api.coinex.com";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// 새 CoinEx 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: true,
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
            logo: Some("https://user-images.githubusercontent.com/51840849/87182089-1e05fa00-c2ec-11ea-8da9-cc73b45c4010.jpg".into()),
            api: api_urls,
            www: Some("https://www.coinex.com".into()),
            doc: vec![
                "https://docs.coinex.com/api/v2/".into(),
            ],
            fees: Some("https://www.coinex.com/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute3, "3min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "1hour".into());
        timeframes.insert(Timeframe::Hour2, "2hour".into());
        timeframes.insert(Timeframe::Hour4, "4hour".into());
        timeframes.insert(Timeframe::Hour6, "6hour".into());
        timeframes.insert(Timeframe::Hour12, "12hour".into());
        timeframes.insert(Timeframe::Day1, "1day".into());
        timeframes.insert(Timeframe::Week1, "1week".into());

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
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Build sorted query string
        let mut query_params = params.clone().unwrap_or_default();
        query_params.insert("access_id".into(), api_key.to_string());
        query_params.insert("tonce".into(), timestamp);

        // Sort params for signature
        let mut sorted_keys: Vec<_> = query_params.keys().collect();
        sorted_keys.sort();

        let query: String = sorted_keys
            .iter()
            .map(|k| format!("{}={}", k, query_params.get(*k).unwrap()))
            .collect::<Vec<_>>()
            .join("&");

        // Sign: query + secret_key
        let sign_str = format!("{query}&secret_key={api_secret}");

        let signature = {
            let digest = md5::compute(sign_str.as_bytes());
            format!("{digest:X}")
        };

        let mut headers = HashMap::new();
        headers.insert("authorization".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => {
                let url = format!("{path}?{query}");
                self.private_client.get(&url, None, Some(headers)).await
            }
            "POST" => {
                let json_body = Some(serde_json::to_value(&query_params).unwrap_or_default());
                self.private_client.post(path, json_body, Some(headers)).await
            }
            "DELETE" => {
                let url = format!("{path}?{query}");
                self.private_client.delete(&url, None, Some(headers)).await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 파싱
    fn parse_market(&self, market_id: &str, data: &CoinexMarket) -> Market {
        let base = data.trading_name.clone().unwrap_or_default();
        let quote = data.pricing_name.clone().unwrap_or_default();
        let symbol = format!("{base}/{quote}");

        Market {
            id: market_id.to_string(),
            lowercase_id: Some(market_id.to_lowercase()),
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
            taker: data.taker_fee_rate.as_ref().and_then(|s| s.parse().ok()),
            maker: data.maker_fee_rate.as_ref().and_then(|s| s.parse().ok()),
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            settle: None,
            settle_id: None,
            precision: MarketPrecision {
                amount: data.trading_decimal,
                price: data.pricing_decimal,
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
    fn parse_ticker(&self, data: &CoinexTicker, symbol: &str) -> Ticker {
        let timestamp = data.date.unwrap_or_else(|| Utc::now().timestamp_millis());

        let ticker_data = &data.ticker;

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: ticker_data.high.as_ref().and_then(|s| s.parse().ok()),
            low: ticker_data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: ticker_data.buy.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: ticker_data.buy_amount.as_ref().and_then(|s| s.parse().ok()),
            ask: ticker_data.sell.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: ticker_data.sell_amount.as_ref().and_then(|s| s.parse().ok()),
            vwap: None,
            open: ticker_data.open.as_ref().and_then(|s| s.parse().ok()),
            close: ticker_data.last.as_ref().and_then(|s| s.parse().ok()),
            last: ticker_data.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: ticker_data.vol.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, data: &CoinexOrder) -> Order {
        let timestamp = data.create_time.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("not_deal") | Some("part_deal") => OrderStatus::Open,
            Some("done") => OrderStatus::Closed,
            Some("cancel") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.order_type.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some(t) if t.contains("limit") => OrderType::Limit,
            Some(t) if t.contains("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|s| s.parse().ok());
        let amount: Decimal = data.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let filled: Decimal = data.deal_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let remaining = data.left.as_ref().and_then(|s| s.parse().ok());
        let cost: Option<Decimal> = data.deal_money.as_ref().and_then(|s| s.parse().ok());

        let market_id = data.market.clone().unwrap_or_default();
        let symbol = self.symbol(&market_id).unwrap_or(market_id);

        Order {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: data.client_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: data.finished_time.map(|t| t * 1000),
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: data.avg_price.as_ref().and_then(|s| s.parse().ok()),
            amount,
            filled,
            remaining,
            cost,
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
    fn parse_trade(&self, data: &CoinexTrade, symbol: &str) -> Trade {
        let timestamp = data.date.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let amount: Decimal = data.amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();

        let side = match data.trade_type.as_deref() {
            Some("buy") => "buy",
            Some("sell") => "sell",
            _ => "buy",
        };

        Trade {
            id: data.id.map(|i| i.to_string()).unwrap_or_default(),
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
    fn parse_ohlcv(&self, data: &[serde_json::Value]) -> Option<OHLCV> {
        if data.len() < 6 {
            return None;
        }
        Some(OHLCV {
            timestamp: data[0].as_i64()? * 1000,
            open: data[1].as_str()?.parse().ok()?,
            close: data[2].as_str()?.parse().ok()?,
            high: data[3].as_str()?.parse().ok()?,
            low: data[4].as_str()?.parse().ok()?,
            volume: data[5].as_str()?.parse().ok()?,
        })
    }

    /// 심볼 변환 (BTC/USDT -> BTCUSDT)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }
}

#[async_trait]
impl Exchange for Coinex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinex
    }

    fn name(&self) -> &str {
        "CoinEx"
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
        let api_key = self.config.api_key().unwrap_or("");
        let api_secret = self.config.secret().unwrap_or("");
        let timestamp = Utc::now().timestamp_millis().to_string();

        let mut query_params = params.clone();
        query_params.insert("access_id".into(), api_key.to_string());
        query_params.insert("tonce".into(), timestamp);

        // Sort params for signature
        let mut sorted_keys: Vec<_> = query_params.keys().collect();
        sorted_keys.sort();

        let query: String = sorted_keys
            .iter()
            .map(|k| format!("{}={}", k, query_params.get(*k).unwrap()))
            .collect::<Vec<_>>()
            .join("&");

        // Sign: query + secret_key
        let sign_str = format!("{query}&secret_key={api_secret}");

        let signature = {
            let digest = md5::compute(sign_str.as_bytes());
            format!("{digest:X}")
        };

        let mut headers = HashMap::new();
        headers.insert("authorization".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        let url = format!("{path}?{query}");

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: CoinexMarketsResponse = self
            .public_get("/v1/market/info", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Markets".to_string(),
            message: "No data in response".into(),
        })?;

        let mut markets = Vec::new();
        for (market_id, market_data) in &data {
            markets.push(self.parse_market(market_id, market_data));
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
        params.insert("market".into(), market_id);

        let response: CoinexTickerResponse = self
            .public_get("/v1/market/ticker", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Ticker".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(self.parse_ticker(&data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CoinexTickersResponse = self
            .public_get("/v1/market/ticker/all", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Tickers".to_string(),
            message: "No data in response".into(),
        })?;

        let timestamp = data.date.unwrap_or_else(|| Utc::now().timestamp_millis());
        let mut result = HashMap::new();

        for (market_id, ticker_inner) in &data.ticker {
            let symbol_str = self.symbol(market_id).unwrap_or_else(|| {
                // Try to convert market_id to symbol (e.g., BTCUSDT -> BTC/USDT)
                market_id.clone()
            });

            if let Some(filter) = symbols {
                if !filter.contains(&symbol_str.as_str()) {
                    continue;
                }
            }

            let ticker_data = CoinexTicker {
                date: Some(timestamp),
                ticker: ticker_inner.clone(),
            };

            let ticker = self.parse_ticker(&ticker_data, &symbol_str);
            result.insert(symbol_str, ticker);
        }

        Ok(result)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexOrderBookResponse = self
            .public_get("/v1/market/depth", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data in response".into(),
        })?;

        let timestamp = data.time.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data.bids.iter()
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

        let asks: Vec<OrderBookEntry> = data.asks.iter()
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
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexTradesResponse = self
            .public_get("/v1/market/deals", Some(params))
            .await?;

        if response.code != 0 {
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
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.convert_to_market_id(symbol);
        let interval = self.timeframes.get(&timeframe).cloned().unwrap_or("1hour".into());

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("type".into(), interval);

        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexKlinesResponse = self
            .public_get("/v1/market/kline", Some(params))
            .await?;

        if response.code != 0 {
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
        let response: CoinexBalanceResponse = self
            .private_request("GET", "/v1/balance/info", None)
            .await?;

        if response.code != 0 {
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

        for (currency, balance_data) in &data {
            let free: Decimal = balance_data.available.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let locked: Decimal = balance_data.frozen.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let total = free + locked;

            balances.currencies.insert(
                currency.clone(),
                Balance {
                    free: Some(free),
                    used: Some(locked),
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
        request.insert("market".into(), market_id);
        request.insert("type".into(), match side {
            OrderSide::Buy => "buy".into(),
            OrderSide::Sell => "sell".into(),
        });
        request.insert("amount".into(), amount.to_string());

        if let Some(p) = price {
            request.insert("price".into(), p.to_string());
        }

        let path = match order_type {
            OrderType::Limit => "/v1/order/limit",
            OrderType::Market => "/v1/order/market",
            _ => "/v1/order/limit",
        };

        let response: CoinexOrderResponse = self
            .private_request("POST", path, Some(request))
            .await?;

        if response.code != 0 {
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

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("id".into(), id.to_string());

        let response: CoinexOrderResponse = self
            .private_request("DELETE", "/v1/order/pending", Some(params))
            .await?;

        if response.code != 0 {
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

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("id".into(), id.to_string());

        let response: CoinexOrderResponse = self
            .private_request("GET", "/v1/order/status", Some(params))
            .await?;

        if response.code != 0 {
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
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("market".into(), self.convert_to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexOrdersResponse = self
            .private_request("GET", "/v1/order/pending", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.data.iter().map(|o| self.parse_order(o)).collect();

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
            params.insert("market".into(), self.convert_to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexMyTradesResponse = self
            .private_request("GET", "/v1/order/user/deals", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Trades".to_string(),
            message: "No data in response".into(),
        })?;

        let sym = symbol.unwrap_or("");
        let trades: Vec<Trade> = data.data.iter().map(|t| self.parse_trade(t, sym)).collect();

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
            params.insert("market".into(), self.convert_to_market_id(s));
        }

        let _response: CoinexResponse<serde_json::Value> = self
            .private_request("DELETE", "/v1/order/pending", if params.is_empty() { None } else { Some(params) })
            .await?;

        Ok(vec![])
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("market".into(), self.convert_to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexOrdersResponse = self
            .private_request("GET", "/v1/order/finished", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.data.iter().map(|o| self.parse_order(o)).collect();
        Ok(orders)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut all_orders = self.fetch_open_orders(symbol, since, limit).await?;
        let closed_orders = self.fetch_closed_orders(symbol, since, limit).await?;
        all_orders.extend(closed_orders);

        all_orders.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        if let Some(l) = limit {
            all_orders.truncate(l as usize);
        }

        Ok(all_orders)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        let response: CoinexCurrenciesResponse = self
            .public_get("/v1/common/asset/config", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let mut currencies = HashMap::new();

        for (code, c) in data {
            let currency = crate::types::Currency {
                id: code.clone(),
                code: code.clone(),
                name: c.asset_name.clone(),
                active: c.deposit_enabled.unwrap_or(false) || c.withdraw_enabled.unwrap_or(false),
                deposit: c.deposit_enabled,
                withdraw: c.withdraw_enabled,
                fee: c.withdraw_tx_fee.clone().and_then(|f| f.parse::<Decimal>().ok()),
                precision: None,
                limits: None,
                networks: HashMap::new(),
                info: serde_json::to_value(&c).unwrap_or_default(),
            };
            currencies.insert(code, currency);
        }

        Ok(currencies)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("coin_type".into(), code.to_string());

        let response: CoinexDepositAddressResponse = self
            .private_request("GET", "/v1/balance/coin/deposit", Some(params))
            .await?;

        if response.code != 0 {
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
            network: None,
            address: data.coin_address.unwrap_or_default(),
            tag: None,
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
            params.insert("coin_type".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexTransactionsResponse = self
            .private_request("GET", "/v1/balance/coin/deposit", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
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
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("coin_type".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: CoinexTransactionsResponse = self
            .private_request("GET", "/v1/balance/coin/withdraw", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
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
        _tag: Option<&str>,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::Transaction> {
        let mut params = HashMap::new();
        params.insert("coin_type".into(), code.to_string());
        params.insert("actual_amount".into(), amount.to_string());
        params.insert("coin_address".into(), address.to_string());

        let response: CoinexWithdrawResponse = self
            .private_request("POST", "/v1/balance/coin/withdraw", Some(params))
            .await?;

        if response.code != 0 {
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
            id: data.coin_withdraw_id.map(|id| id.to_string()).unwrap_or_default(),
            txid: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            address: Some(address.to_string()),
            tag: None,
            tx_type: crate::types::TransactionType::Withdrawal,
            amount,
            currency: code.to_string(),
            status: crate::types::TransactionStatus::Pending,
            updated: None,
            fee: None,
            network: None,
            internal: None,
            confirmations: None,
        })
    }
}

impl Coinex {
    fn parse_transaction(&self, data: &CoinexTransaction, transaction_type: &str) -> crate::types::Transaction {
        let amount = data.actual_amount
            .as_ref()
            .and_then(|a| a.parse::<Decimal>().ok())
            .unwrap_or_default();

        let status = match data.status.as_deref() {
            Some("finish") | Some("completed") => crate::types::TransactionStatus::Ok,
            Some("processing") | Some("pending") => crate::types::TransactionStatus::Pending,
            Some("fail") | Some("failed") => crate::types::TransactionStatus::Failed,
            Some("cancel") | Some("cancelled") => crate::types::TransactionStatus::Canceled,
            _ => crate::types::TransactionStatus::Pending,
        };

        crate::types::Transaction {
            info: serde_json::to_value(data).unwrap_or_default(),
            id: data.coin_deposit_id.map(|id| id.to_string())
                .or_else(|| data.coin_withdraw_id.map(|id| id.to_string()))
                .unwrap_or_default(),
            txid: data.tx_id.clone(),
            timestamp: data.create_time,
            datetime: data.create_time.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            address: data.coin_address.clone(),
            tag: None,
            tx_type: if transaction_type == "deposit" {
                crate::types::TransactionType::Deposit
            } else {
                crate::types::TransactionType::Withdrawal
            },
            amount,
            currency: data.coin_type.clone().unwrap_or_default(),
            status,
            updated: None,
            fee: None,
            network: None,
            internal: None,
            confirmations: data.confirmations.map(|c| c as u32),
        }
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct CoinexResponse<T> {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

type CoinexMarketsResponse = CoinexResponse<HashMap<String, CoinexMarket>>;
type CoinexTickerResponse = CoinexResponse<CoinexTicker>;
type CoinexOrderBookResponse = CoinexResponse<CoinexOrderBookData>;
type CoinexTradesResponse = CoinexResponse<Vec<CoinexTrade>>;
type CoinexKlinesResponse = CoinexResponse<Vec<Vec<serde_json::Value>>>;
type CoinexBalanceResponse = CoinexResponse<HashMap<String, CoinexBalance>>;
type CoinexOrderResponse = CoinexResponse<CoinexOrder>;
type CoinexOrdersResponse = CoinexResponse<CoinexOrdersData>;
type CoinexMyTradesResponse = CoinexResponse<CoinexMyTradesData>;
type CoinexCurrenciesResponse = CoinexResponse<HashMap<String, CoinexCurrencyInfo>>;
type CoinexDepositAddressResponse = CoinexResponse<CoinexDepositAddressData>;
type CoinexTransactionsResponse = CoinexResponse<Vec<CoinexTransaction>>;
type CoinexWithdrawResponse = CoinexResponse<CoinexWithdrawData>;

#[derive(Debug, Default, Deserialize)]
struct CoinexTickersResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<CoinexTickersData>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexTickersData {
    #[serde(default)]
    date: Option<i64>,
    #[serde(default)]
    ticker: HashMap<String, CoinexTickerInner>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct CoinexMarket {
    #[serde(default)]
    trading_name: Option<String>,
    #[serde(default)]
    pricing_name: Option<String>,
    #[serde(default)]
    trading_decimal: Option<i32>,
    #[serde(default)]
    pricing_decimal: Option<i32>,
    #[serde(default)]
    taker_fee_rate: Option<String>,
    #[serde(default)]
    maker_fee_rate: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexTicker {
    #[serde(default)]
    date: Option<i64>,
    #[serde(default)]
    ticker: CoinexTickerInner,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct CoinexTickerInner {
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    buy: Option<String>,
    #[serde(default)]
    buy_amount: Option<String>,
    #[serde(default)]
    sell: Option<String>,
    #[serde(default)]
    sell_amount: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
    #[serde(default)]
    time: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    trade_type: Option<String>,
    #[serde(default)]
    date: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexBalance {
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexOrder {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    deal_amount: Option<String>,
    #[serde(default)]
    deal_money: Option<String>,
    #[serde(default)]
    left: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    finished_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexOrdersData {
    #[serde(default)]
    data: Vec<CoinexOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct CoinexMyTradesData {
    #[serde(default)]
    data: Vec<CoinexTrade>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexCurrencyInfo {
    #[serde(default)]
    asset_name: Option<String>,
    #[serde(default)]
    deposit_enabled: Option<bool>,
    #[serde(default)]
    withdraw_enabled: Option<bool>,
    #[serde(default)]
    withdraw_tx_fee: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexDepositAddressData {
    #[serde(default)]
    coin_address: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexTransaction {
    #[serde(default)]
    coin_deposit_id: Option<i64>,
    #[serde(default)]
    coin_withdraw_id: Option<i64>,
    #[serde(default)]
    coin_type: Option<String>,
    #[serde(default)]
    actual_amount: Option<String>,
    #[serde(default)]
    coin_address: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    confirmations: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CoinexWithdrawData {
    #[serde(default)]
    coin_withdraw_id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Coinex::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Coinex);
        assert_eq!(exchange.name(), "CoinEx");
        assert!(exchange.has().spot);
        assert!(exchange.has().swap);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Coinex::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USDT"), "BTCUSDT");
    }
}
