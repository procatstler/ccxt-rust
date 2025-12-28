//! BingX Exchange Implementation
//!
//! BingX 거래소 구현체

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

/// BingX 거래소
pub struct Bingx {
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

impl Bingx {
    const BASE_URL: &'static str = "https://open-api.bingx.com";
    const RATE_LIMIT_MS: u64 = 50; // 20 requests per second

    /// 새 BingX 인스턴스 생성
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
            logo: Some("https://github.com/user-attachments/assets/bingx-logo.png".into()),
            api: api_urls,
            www: Some("https://bingx.com".into()),
            doc: vec![
                "https://bingx-api.github.io/docs/".into(),
            ],
            fees: Some("https://bingx.com/en-us/fee/".into()),
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

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Build query string with timestamp
        let mut query_params = params.clone().unwrap_or_default();
        query_params.insert("timestamp".into(), timestamp);

        let query: String = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Sign with HMAC-SHA256
        let signature = {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };

        let signed_query = format!("{query}&signature={signature}");

        let mut headers = HashMap::new();
        headers.insert("X-BX-APIKEY".into(), api_key.to_string());

        let url = format!("{path}?{signed_query}");

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
                self.private_client.post(&format!("{path}?{signed_query}"), None, Some(headers)).await
            }
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 파싱
    fn parse_market(&self, data: &BingxSymbol) -> Market {
        let symbol = format!("{}/{}", data.base_asset, data.quote_asset);

        Market {
            id: data.symbol.clone(),
            lowercase_id: Some(data.symbol.to_lowercase()),
            symbol: symbol.clone(),
            base: data.base_asset.clone(),
            quote: data.quote_asset.clone(),
            base_id: data.base_asset.clone(),
            quote_id: data.quote_asset.clone(),
            market_type: MarketType::Spot,
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
            index: false,
            active: data.status.as_deref() == Some("1"),
            contract: false,
            linear: None,
            inverse: None,
            sub_type: None,
            taker: data.taker_commission.as_ref().and_then(|s| s.parse().ok()),
            maker: data.maker_commission.as_ref().and_then(|s| s.parse().ok()),
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            settle: None,
            settle_id: None,
            precision: MarketPrecision {
                amount: data.quantity_precision,
                price: data.price_precision,
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
    fn parse_ticker(&self, data: &BingxTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price.as_ref().and_then(|s| s.parse().ok()),
            low: data.low_price.as_ref().and_then(|s| s.parse().ok()),
            bid: data.bid_price.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: data.bid_qty.as_ref().and_then(|s| s.parse().ok()),
            ask: data.ask_price.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: data.ask_qty.as_ref().and_then(|s| s.parse().ok()),
            vwap: None,
            open: data.open_price.as_ref().and_then(|s| s.parse().ok()),
            close: data.last_price.as_ref().and_then(|s| s.parse().ok()),
            last: data.last_price.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: data.price_change.as_ref().and_then(|s| s.parse().ok()),
            percentage: data.price_change_percent.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: data.quote_volume.as_ref().and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, data: &BingxOrder) -> Order {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("NEW") | Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELED") | Some("REJECTED") | Some("EXPIRED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("STOP_MARKET") => OrderType::StopMarket,
            Some("STOP_LIMIT") | Some("STOP") => OrderType::StopLimit,
            Some("TAKE_PROFIT") | Some("TAKE_PROFIT_MARKET") => OrderType::TakeProfit,
            _ => OrderType::Limit,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|s| s.parse().ok());
        let amount: Decimal = data.orig_qty.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let filled: Decimal = data.executed_qty.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let remaining = amount - filled;
        let cost: Option<Decimal> = data.cum_quote.as_ref().and_then(|s| s.parse().ok());

        let symbol = data.symbol.clone().unwrap_or_default();
        let unified_symbol = self.symbol(&symbol).unwrap_or(symbol.clone());

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
            symbol: unified_symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: if filled > Decimal::ZERO {
                cost.map(|c| c / filled)
            } else {
                None
            },
            amount,
            filled,
            remaining: Some(remaining),
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: data.stop_price.as_ref().and_then(|s| s.parse().ok()),
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        }
    }

    /// 거래 파싱
    fn parse_trade(&self, data: &BingxTrade, symbol: &str) -> Trade {
        let timestamp = data.time.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.price.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
        let amount: Decimal = data.qty.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();

        let side = if data.is_buyer_maker.unwrap_or(false) {
            "sell"
        } else {
            "buy"
        };

        Trade {
            id: data.id.clone().unwrap_or_default(),
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
    fn parse_ohlcv(&self, data: &BingxKline) -> Option<OHLCV> {
        Some(OHLCV {
            timestamp: data.time?,
            open: data.open.as_ref()?.parse().ok()?,
            high: data.high.as_ref()?.parse().ok()?,
            low: data.low.as_ref()?.parse().ok()?,
            close: data.close.as_ref()?.parse().ok()?,
            volume: data.volume.as_ref()?.parse().ok()?,
        })
    }

    /// 심볼 변환 (BTC/USDT -> BTC-USDT)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 마켓 ID를 심볼로 변환 (BTC-USDT -> BTC/USDT)
    fn convert_to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }
}

#[async_trait]
impl Exchange for Bingx {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bingx
    }

    fn name(&self) -> &str {
        "BingX"
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

        let api_key = self.config.api_key().unwrap_or("");
        let api_secret = self.config.secret().unwrap_or("");
        let timestamp = Utc::now().timestamp_millis().to_string();

        let mut query_params = params.clone();
        query_params.insert("timestamp".into(), timestamp);

        let query: String = query_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let signature = if !api_secret.is_empty() {
            let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        } else {
            String::new()
        };

        let signed_query = format!("{query}&signature={signature}");

        let mut headers = HashMap::new();
        headers.insert("X-BX-APIKEY".into(), api_key.to_string());

        SignedRequest {
            url: format!("{path}?{signed_query}"),
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let response: BingxMarketsResponse = self
            .public_get("/openApi/spot/v1/common/symbols", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Markets".to_string(),
            message: "No data in response".into(),
        })?;

        let mut markets = Vec::new();
        for symbol in &data.symbols {
            markets.push(self.parse_market(symbol));
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
        params.insert("symbol".into(), market_id);

        let response: BingxTickerResponse = self
            .public_get("/openApi/spot/v1/ticker/24hr", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Ticker".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(self.parse_ticker(&data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BingxTickersResponse = self
            .public_get("/openApi/spot/v1/ticker/24hr", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Tickers".to_string(),
            message: "No data in response".into(),
        })?;

        let mut result = HashMap::new();
        for ticker_data in &data {
            let market_id = ticker_data.symbol.clone().unwrap_or_default();
            let symbol_str = self.convert_to_symbol(&market_id);

            if let Some(filter) = symbols {
                if filter.contains(&symbol_str.as_str()) {
                    let ticker = self.parse_ticker(ticker_data, &symbol_str);
                    result.insert(symbol_str, ticker);
                }
            } else {
                let ticker = self.parse_ticker(ticker_data, &symbol_str);
                result.insert(symbol_str, ticker);
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

        let response: BingxOrderBookResponse = self
            .public_get("/openApi/spot/v1/market/depth", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "OrderBook".to_string(),
            message: "No data in response".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis();

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
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BingxTradesResponse = self
            .public_get("/openApi/spot/v1/market/trades", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        let interval = self.timeframes.get(&timeframe).cloned().unwrap_or("1h".into());

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BingxKlinesResponse = self
            .public_get("/openApi/spot/v1/market/kline", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        let response: BingxBalanceResponse = self
            .private_request("GET", "/openApi/spot/v1/account/balance", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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

        for balance in data.balances {
            let free: Decimal = balance.free.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let locked: Decimal = balance.locked.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let total = free + locked;

            balances.currencies.insert(
                balance.asset.clone(),
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
        request.insert("symbol".into(), market_id);
        request.insert("side".into(), match side {
            OrderSide::Buy => "BUY".into(),
            OrderSide::Sell => "SELL".into(),
        });
        request.insert("type".into(), match order_type {
            OrderType::Limit => "LIMIT".into(),
            OrderType::Market => "MARKET".into(),
            _ => "LIMIT".into(),
        });
        request.insert("quantity".into(), amount.to_string());

        if let Some(p) = price {
            request.insert("price".into(), p.to_string());
        }

        let response: BingxOrderResponse = self
            .private_request("POST", "/openApi/spot/v1/trade/order", Some(request))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BingxOrderResponse = self
            .private_request("POST", "/openApi/spot/v1/trade/cancel", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        params.insert("orderId".into(), id.to_string());

        let response: BingxOrderResponse = self
            .private_request("GET", "/openApi/spot/v1/trade/query", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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

        let response: BingxOrdersResponse = self
            .private_request("GET", "/openApi/spot/v1/trade/openOrders", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BingxMyTradesResponse = self
            .private_request("GET", "/openApi/spot/v1/trade/myTrades", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Trades".to_string(),
            message: "No data in response".into(),
        })?;

        let sym = symbol.unwrap_or("");
        let trades: Vec<Trade> = data.fills.iter().map(|t| self.parse_trade(t, sym)).collect();

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

        let _response: BingxResponse<serde_json::Value> = self
            .private_request("POST", "/openApi/spot/v1/trade/cancelOpenOrders", if params.is_empty() { None } else { Some(params) })
            .await?;

        // BingX doesn't return detailed order info on cancel all
        Ok(vec![])
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".into(), self.convert_to_market_id(s));
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BingxOrdersResponse = self
            .private_request("GET", "/openApi/spot/v1/trade/historyOrders", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Orders".to_string(),
            message: "No data in response".into(),
        })?;

        let orders: Vec<Order> = data.orders.iter().map(|o| self.parse_order(o)).collect();
        Ok(orders)
    }

    async fn fetch_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        // Combine open and closed orders
        let mut all_orders = self.fetch_open_orders(symbol, since, limit).await?;
        let closed_orders = self.fetch_closed_orders(symbol, since, limit).await?;
        all_orders.extend(closed_orders);

        // Sort by timestamp
        all_orders.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        if let Some(l) = limit {
            all_orders.truncate(l as usize);
        }

        Ok(all_orders)
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, crate::types::Currency>> {
        let response: BingxCurrenciesResponse = self
            .public_get("/openApi/wallets/v1/capital/config/getall", None)
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let mut currencies = HashMap::new();

        for c in data {
            let currency = crate::types::Currency {
                id: c.coin.clone(),
                code: c.coin.clone(),
                name: c.name.clone(),
                active: true,
                deposit: Some(c.deposit_enable.unwrap_or(false)),
                withdraw: Some(c.withdraw_enable.unwrap_or(false)),
                fee: None,
                precision: None,
                limits: None,
                networks: HashMap::new(),
                info: serde_json::to_value(&c).unwrap_or_default(),
            };
            currencies.insert(c.coin.clone(), currency);
        }

        Ok(currencies)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_string());
        if let Some(n) = network {
            params.insert("network".into(), n.to_string());
        }

        let response: BingxDepositAddressResponse = self
            .private_request("GET", "/openApi/wallets/v1/capital/deposit/address", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "DepositAddress".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(crate::types::DepositAddress {
            info: serde_json::to_value(&data).unwrap_or_default(),
            currency: code.to_string(),
            network: data.network,
            address: data.address.unwrap_or_default(),
            tag: data.tag,
        })
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("coin".into(), c.to_string());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BingxTransactionsResponse = self
            .private_request("GET", "/openApi/wallets/v1/capital/deposit/hisrec", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<crate::types::Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("coin".into(), c.to_string());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BingxTransactionsResponse = self
            .private_request("GET", "/openApi/wallets/v1/capital/withdraw/history", if params.is_empty() { None } else { Some(params) })
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
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
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<crate::types::Transaction> {
        let mut params = HashMap::new();
        params.insert("coin".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("addressTag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("network".into(), n.to_string());
        }

        let response: BingxWithdrawResponse = self
            .private_request("POST", "/openApi/wallets/v1/capital/withdraw/apply", Some(params))
            .await?;

        if response.code != 0 {
            return Err(CcxtError::ExchangeError {
                message: response.msg.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or(CcxtError::ParseError {
            data_type: "Withdrawal".to_string(),
            message: "No data in response".into(),
        })?;

        Ok(crate::types::Transaction {
            info: serde_json::to_value(&data).unwrap_or_default(),
            id: data.id.unwrap_or_default(),
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

impl Bingx {
    fn parse_transaction(&self, data: &BingxTransaction, transaction_type: &str) -> crate::types::Transaction {
        let amount = data.amount
            .as_ref()
            .and_then(|a| a.parse::<Decimal>().ok())
            .unwrap_or_default();

        let status = match data.status.as_deref() {
            Some("1") | Some("completed") => crate::types::TransactionStatus::Ok,
            Some("0") | Some("pending") => crate::types::TransactionStatus::Pending,
            Some("2") | Some("failed") => crate::types::TransactionStatus::Failed,
            Some("3") | Some("cancelled") => crate::types::TransactionStatus::Canceled,
            _ => crate::types::TransactionStatus::Pending,
        };

        crate::types::Transaction {
            info: serde_json::to_value(data).unwrap_or_default(),
            id: data.id.clone().unwrap_or_default(),
            txid: data.txid.clone(),
            timestamp: data.insert_time,
            datetime: data.insert_time.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t).map(|dt| dt.to_rfc3339())
            }),
            address: data.address.clone(),
            tag: data.address_tag.clone(),
            tx_type: if transaction_type == "deposit" {
                crate::types::TransactionType::Deposit
            } else {
                crate::types::TransactionType::Withdrawal
            },
            amount,
            currency: data.coin.clone().unwrap_or_default(),
            status,
            updated: None,
            fee: None,
            network: data.network.clone(),
            internal: None,
            confirmations: data.confirm_times.map(|c| c as u32),
        }
    }
}

// === Response Types ===

#[derive(Debug, Default, Deserialize)]
struct BingxResponse<T> {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

type BingxMarketsResponse = BingxResponse<BingxMarketsData>;
type BingxTickerResponse = BingxResponse<BingxTicker>;
type BingxTickersResponse = BingxResponse<Vec<BingxTicker>>;
type BingxOrderBookResponse = BingxResponse<BingxOrderBookData>;
type BingxTradesResponse = BingxResponse<Vec<BingxTrade>>;
type BingxKlinesResponse = BingxResponse<Vec<BingxKline>>;
type BingxBalanceResponse = BingxResponse<BingxBalanceData>;
type BingxOrderResponse = BingxResponse<BingxOrder>;
type BingxOrdersResponse = BingxResponse<BingxOrdersData>;
type BingxMyTradesResponse = BingxResponse<BingxMyTradesData>;
type BingxCurrenciesResponse = BingxResponse<Vec<BingxCurrency>>;
type BingxDepositAddressResponse = BingxResponse<BingxDepositAddressData>;
type BingxTransactionsResponse = BingxResponse<Vec<BingxTransaction>>;
type BingxWithdrawResponse = BingxResponse<BingxWithdrawData>;

#[derive(Debug, Default, Deserialize)]
struct BingxMarketsData {
    #[serde(default)]
    symbols: Vec<BingxSymbol>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxSymbol {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    base_asset: String,
    #[serde(default)]
    quote_asset: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price_precision: Option<i32>,
    #[serde(default)]
    quantity_precision: Option<i32>,
    #[serde(default)]
    taker_commission: Option<String>,
    #[serde(default)]
    maker_commission: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxTicker {
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    last_price: Option<String>,
    #[serde(default)]
    bid_price: Option<String>,
    #[serde(default)]
    bid_qty: Option<String>,
    #[serde(default)]
    ask_price: Option<String>,
    #[serde(default)]
    ask_qty: Option<String>,
    #[serde(default)]
    open_price: Option<String>,
    #[serde(default)]
    high_price: Option<String>,
    #[serde(default)]
    low_price: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    quote_volume: Option<String>,
    #[serde(default)]
    price_change: Option<String>,
    #[serde(default)]
    price_change_percent: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxOrderBookData {
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxTrade {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    qty: Option<String>,
    #[serde(default)]
    is_buyer_maker: Option<bool>,
    #[serde(default)]
    time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BingxKline {
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxBalanceData {
    #[serde(default)]
    balances: Vec<BingxBalance>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxBalance {
    #[serde(default)]
    asset: String,
    #[serde(default)]
    free: Option<String>,
    #[serde(default)]
    locked: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    orig_qty: Option<String>,
    #[serde(default)]
    executed_qty: Option<String>,
    #[serde(default)]
    cum_quote: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxOrdersData {
    #[serde(default)]
    orders: Vec<BingxOrder>,
}

#[derive(Debug, Default, Deserialize)]
struct BingxMyTradesData {
    #[serde(default)]
    fills: Vec<BingxTrade>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxCurrency {
    #[serde(default)]
    coin: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    deposit_enable: Option<bool>,
    #[serde(default)]
    withdraw_enable: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxDepositAddressData {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    network: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BingxTransaction {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    coin: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_tag: Option<String>,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    network: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    insert_time: Option<i64>,
    #[serde(default)]
    confirm_times: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BingxWithdrawData {
    #[serde(default)]
    id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bingx::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bingx);
        assert_eq!(exchange.name(), "BingX");
        assert!(exchange.has().spot);
        assert!(exchange.has().swap);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bingx::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/USDT"), "BTC-USDT");
        assert_eq!(exchange.convert_to_symbol("BTC-USDT"), "BTC/USDT");
    }
}
