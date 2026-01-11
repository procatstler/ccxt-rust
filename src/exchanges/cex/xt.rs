//! XT Exchange Implementation
//!
//! CCXT xt.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Transaction,
    TransactionStatus, TransactionType, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// XT 거래소
pub struct Xt {
    config: ExchangeConfig,
    spot_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Xt {
    const SPOT_URL: &'static str = "https://sapi.xt.com";
    const RATE_LIMIT_MS: u64 = 100;

    /// 새 XT 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let spot_client = HttpClient::new(Self::SPOT_URL, &config)?;
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
            edit_order: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("spot".into(), Self::SPOT_URL.into());
        api_urls.insert("linear".into(), "https://fapi.xt.com".into());
        api_urls.insert("inverse".into(), "https://dapi.xt.com".into());
        api_urls.insert("user".into(), "https://api.xt.com".into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/14319357/232636712-466df2fc-560a-4ca4-aab2-b1d954a58e24.jpg".into()),
            api: api_urls,
            www: Some("https://xt.com".into()),
            doc: vec![
                "https://doc.xt.com/".into(),
                "https://github.com/xtpub/api-doc".into(),
            ],
            fees: Some("https://www.xt.com/en/rate".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Hour6, "6h".into());
        timeframes.insert(Timeframe::Hour8, "8h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());
        timeframes.insert(Timeframe::Day3, "3d".into());
        timeframes.insert(Timeframe::Week1, "1w".into());
        timeframes.insert(Timeframe::Month1, "1M".into());

        Ok(Self {
            config,
            spot_client,
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

        self.spot_client.get(&url, None, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let timestamp = Utc::now().timestamp_millis().to_string();

        // Build query string or body
        let (query_string, body) = if method == "GET" || method == "DELETE" {
            let query = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            (query, String::new())
        } else {
            let body = serde_json::to_string(&params).unwrap_or_default();
            (String::new(), body)
        };

        // Create signature: timestamp + method + path + query/body
        let sign_string = format!("{timestamp}#{method}#{path}#{}", if query_string.is_empty() { &body } else { &query_string });

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("xt-validate-appkey".into(), api_key.to_string());
        headers.insert("xt-validate-timestamp".into(), timestamp);
        headers.insert("xt-validate-signature".into(), signature);
        headers.insert("xt-validate-recvwindow".into(), "5000".into());
        headers.insert("xt-validate-algorithms".into(), "HmacSHA256".into());
        headers.insert("Content-Type".into(), "application/json".into());

        let url = if query_string.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query_string}")
        };

        match method {
            "GET" => self.spot_client.get(&url, None, Some(headers)).await,
            "POST" => {
                let json_body: Option<serde_json::Value> = if body.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body).unwrap_or_default())
                };
                self.spot_client.post(&url, json_body, Some(headers)).await
            }
            "DELETE" => self.spot_client.delete(&url, None, Some(headers)).await,
            "PUT" => {
                let json_body: Option<serde_json::Value> = if body.is_empty() {
                    None
                } else {
                    Some(serde_json::from_str(&body).unwrap_or_default())
                };
                self.spot_client.put_json(&url, json_body, Some(headers)).await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 변환 (BTC/USDT -> btc_usdt)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &XtTicker, symbol: &str) -> Ticker {
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.h.as_ref().and_then(|v| v.parse().ok()),
            low: data.l.as_ref().and_then(|v| v.parse().ok()),
            bid: data.b.as_ref().and_then(|v| v.parse().ok()),
            bid_volume: data.bv.as_ref().and_then(|v| v.parse().ok()),
            ask: data.a.as_ref().and_then(|v| v.parse().ok()),
            ask_volume: data.av.as_ref().and_then(|v| v.parse().ok()),
            vwap: None,
            open: data.o.as_ref().and_then(|v| v.parse().ok()),
            close: data.c.as_ref().and_then(|v| v.parse().ok()),
            last: data.c.as_ref().and_then(|v| v.parse().ok()),
            previous_close: None,
            change: data.ch.as_ref().and_then(|v| v.parse().ok()),
            percentage: data.p.as_ref().and_then(|v| v.parse().ok()),
            average: None,
            base_volume: data.v.as_ref().and_then(|v| v.parse().ok()),
            quote_volume: data.q.as_ref().and_then(|v| v.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &XtOrder, symbol: &str) -> Order {
        let status = match data.state.as_deref() {
            Some("NEW") => OrderStatus::Open,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELED") => OrderStatus::Canceled,
            Some("PARTIALLY_CANCELED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.order_side.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.orig_qty.as_ref().and_then(|q| q.parse().ok()).unwrap_or_default();
        let filled: Decimal = data.executed_qty.as_ref().and_then(|q| q.parse().ok()).unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data.cum_quote.as_ref().and_then(|c| c.parse().ok());
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        Order {
            id: data.order_id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.create_time,
            datetime: data.create_time.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: data.update_time,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average,
            amount,
            filled,
            remaining,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[XtBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.available.as_ref().and_then(|f| f.parse().ok());
            let frozen: Option<Decimal> = b.frozen.as_ref().and_then(|f| f.parse().ok());
            let total = match (free, frozen) {
                (Some(f), Some(u)) => Some(f + u),
                _ => None,
            };

            let balance = Balance {
                free,
                used: frozen,
                total,
                debt: None,
            };
            result.add(&b.currency, balance);
        }

        result
    }

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &XtTrade, symbol: Option<&str>) -> Trade {
        let timestamp = data.t.unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data.p.as_ref().and_then(|p| p.parse().ok()).unwrap_or_default();
        let amount: Decimal = data.v.as_ref().and_then(|v| v.parse().ok()).unwrap_or_default();

        let symbol_str = symbol.map(|s| s.to_string()).unwrap_or_else(|| {
            data.s.clone().unwrap_or_default()
        });

        Trade {
            id: data.i.clone().unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol_str,
            trade_type: None,
            side: data.m.as_ref().map(|m| if *m { "sell" } else { "buy" }.into()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &XtDeposit) -> Transaction {
        let status = match data.state.as_deref() {
            Some("COMPLETED") | Some("SUCCESS") => TransactionStatus::Ok,
            Some("FAILED") => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        let amount: Decimal = data.amount.as_ref().and_then(|a| a.parse().ok()).unwrap_or_default();

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp: data.created_at,
            datetime: data.created_at.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: data.updated_at,
            tx_type: TransactionType::Deposit,
            currency: data.currency.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.address.clone(),
            tag: data.address_memo.clone(),
            txid: data.tx_id.clone(),
            fee: None,
            internal: None,
            confirmations: data.confirmations.and_then(|c| u32::try_from(c).ok()),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &XtWithdrawal) -> Transaction {
        let status = match data.state.as_deref() {
            Some("COMPLETED") | Some("SUCCESS") => TransactionStatus::Ok,
            Some("FAILED") => TransactionStatus::Failed,
            _ => TransactionStatus::Pending,
        };

        let amount: Decimal = data.amount.as_ref().and_then(|a| a.parse().ok()).unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|f| f.parse().ok());

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp: data.created_at,
            datetime: data.created_at.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: data.updated_at,
            tx_type: TransactionType::Withdrawal,
            currency: data.currency.clone(),
            network: data.chain.clone(),
            amount,
            status,
            address: data.address.clone(),
            tag: data.address_memo.clone(),
            txid: data.tx_id.clone(),
            fee: fee_amount.map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: Some(data.currency.clone()),
                rate: None,
            }),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Xt {
    fn id(&self) -> ExchangeId {
        ExchangeId::Xt
    }

    fn name(&self) -> &str {
        "XT"
    }

    fn version(&self) -> &str {
        "v4"
    }

    fn countries(&self) -> &[&str] {
        &["SC"] // Seychelles
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
        let response: XtSymbolsResponse = self.public_get("/v4/public/symbol", None).await?;

        let mut markets = Vec::new();

        if let Some(symbols) = response.result {
            for symbol_info in symbols {
                if let Some(state) = symbol_info.state.as_deref() {
                    if state != "TRADING" {
                        continue;
                    }
                }

                let id = symbol_info.symbol.clone();
                let base = symbol_info.base_currency.clone();
                let quote = symbol_info.quote_currency.clone();
                let symbol = format!("{base}/{quote}");

                let market = Market {
                    id: id.clone(),
                    lowercase_id: Some(id.to_lowercase()),
                    symbol: symbol.clone(),
                    base: base.clone(),
                    quote: quote.clone(),
                    base_id: base.clone(),
                    quote_id: quote.clone(),
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
                    taker: Some(Decimal::new(2, 3)), // 0.002
                    maker: Some(Decimal::new(2, 3)), // 0.002
                    contract_size: None,
                    expiry: None,
                    expiry_datetime: None,
                    strike: None,
                    option_type: None,
                    precision: MarketPrecision {
                        amount: symbol_info.base_precision,
                        price: symbol_info.quote_precision,
                        cost: None,
                        base: symbol_info.base_precision,
                        quote: symbol_info.quote_precision,
                    },
                    limits: MarketLimits::default(),
                    margin_modes: None,
                    created: None,
                    info: serde_json::to_value(&symbol_info).unwrap_or_default(),
                    tier_based: false,
                    percentage: true,
                };

                markets.push(market);
            }
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: XtTickerResponse = self
            .public_get("/v4/public/ticker/24h", Some(params))
            .await?;

        if let Some(result) = response.result.and_then(|r| r.first().cloned()) {
            Ok(self.parse_ticker(&result, symbol))
        } else {
            Err(CcxtError::BadSymbol { symbol: symbol.into() })
        }
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: XtTickerResponse = self.public_get("/v4/public/ticker/24h", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        if let Some(results) = response.result {
            for data in results {
                if let Some(market_id) = data.s.as_ref() {
                    if let Some(symbol) = markets_by_id.get(market_id) {
                        if let Some(filter) = symbols {
                            if !filter.contains(&symbol.as_str()) {
                                continue;
                            }
                        }

                        let ticker = self.parse_ticker(&data, symbol);
                        tickers.insert(symbol.clone(), ticker);
                    }
                }
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: XtOrderBookResponse = self
            .public_get("/v4/public/depth", Some(params))
            .await?;

        if let Some(result) = response.result {
            let bids: Vec<OrderBookEntry> = result.bids.iter()
                .filter_map(|b| {
                    if b.len() >= 2 {
                        Some(OrderBookEntry {
                            price: b[0].parse().ok()?,
                            amount: b[1].parse().ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect();

            let asks: Vec<OrderBookEntry> = result.asks.iter()
                .filter_map(|a| {
                    if a.len() >= 2 {
                        Some(OrderBookEntry {
                            price: a[0].parse().ok()?,
                            amount: a[1].parse().ok()?,
                        })
                    } else {
                        None
                    }
                })
                .collect();

            Ok(OrderBook {
                symbol: symbol.to_string(),
                timestamp: result.timestamp,
                datetime: result.timestamp.and_then(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                }),
                nonce: None,
                bids,
                asks,
            })
        } else {
            Err(CcxtError::BadResponse { message: "Empty orderbook response".into() })
        }
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(500).to_string());
        }

        let response: XtTradesResponse = self
            .public_get("/v4/public/trade/recent", Some(params))
            .await?;

        if let Some(results) = response.result {
            let trades: Vec<Trade> = results.iter()
                .map(|t| self.parse_trade(t, Some(symbol)))
                .collect();
            Ok(trades)
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let interval = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {timeframe:?}"),
        })?;

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("interval".into(), interval.clone());
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: XtKlineResponse = self
            .public_get("/v4/public/kline", Some(params))
            .await?;

        if let Some(results) = response.result {
            let ohlcv: Vec<OHLCV> = results.iter()
                .filter_map(|c| {
                    if c.len() < 6 {
                        return None;
                    }
                    Some(OHLCV {
                        timestamp: c[0].parse().ok()?,
                        open: c[1].parse().ok()?,
                        high: c[2].parse().ok()?,
                        low: c[3].parse().ok()?,
                        close: c[4].parse().ok()?,
                        volume: c[5].parse().ok()?,
                    })
                })
                .collect();
            Ok(ohlcv)
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: XtBalanceResponse = self
            .private_request("GET", "/v4/balances", HashMap::new())
            .await?;

        if let Some(result) = response.result {
            Ok(self.parse_balance(&result.assets))
        } else {
            Ok(Balances::new())
        }
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
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
        params.insert("quantity".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
        }

        let response: XtOrderResponse = self
            .private_request("POST", "/v4/order", params)
            .await?;

        if let Some(result) = response.result {
            Ok(self.parse_order(&result, symbol))
        } else {
            Err(CcxtError::BadResponse { message: "Empty order response".into() })
        }
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let _market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let path = format!("/v4/order/{id}");
        let response: XtOrderResponse = self
            .private_request("DELETE", &path, params)
            .await?;

        if let Some(result) = response.result {
            Ok(self.parse_order(&result, symbol))
        } else {
            Err(CcxtError::BadResponse { message: "Empty cancel response".into() })
        }
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let _market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let path = format!("/v4/order/{id}");
        let response: XtOrderResponse = self
            .private_request("GET", &path, params)
            .await?;

        if let Some(result) = response.result {
            Ok(self.parse_order(&result, symbol))
        } else {
            Err(CcxtError::BadResponse { message: "Empty order response".into() })
        }
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: XtOrdersResponse = self
            .private_request("GET", "/v4/open-order", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        if let Some(results) = response.result {
            let orders: Vec<Order> = results.iter()
                .map(|o| {
                    let sym = markets_by_id
                        .get(&o.symbol.clone().unwrap_or_default())
                        .cloned()
                        .unwrap_or_else(|| o.symbol.clone().unwrap_or_default());
                    self.parse_order(o, &sym)
                })
                .collect();
            Ok(orders)
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();

        if let Some(s) = symbol {
            params.insert("symbol".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: XtMyTradesResponse = self
            .private_request("GET", "/v4/trade", params)
            .await?;

        if let Some(results) = response.result {
            let trades: Vec<Trade> = results.iter()
                .map(|t| self.parse_trade(t, symbol))
                .collect();
            Ok(trades)
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: XtDepositsResponse = self
            .private_request("GET", "/v4/deposit/history", params)
            .await?;

        if let Some(results) = response.result {
            let transactions: Vec<Transaction> = results.iter()
                .map(|d| self.parse_deposit(d))
                .collect();
            Ok(transactions)
        } else {
            Ok(Vec::new())
        }
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("currency".into(), c.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: XtWithdrawalsResponse = self
            .private_request("GET", "/v4/withdraw/history", params)
            .await?;

        if let Some(results) = response.result {
            let transactions: Vec<Transaction> = results.iter()
                .map(|w| self.parse_withdrawal(w))
                .collect();
            Ok(transactions)
        } else {
            Ok(Vec::new())
        }
    }

    async fn withdraw(
        &self,
        code: &str,
        amount: Decimal,
        address: &str,
        tag: Option<&str>,
        network: Option<&str>,
    ) -> CcxtResult<Transaction> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("addressMemo".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: XtWithdrawResponse = self
            .private_request("POST", "/v4/withdraw", params)
            .await?;

        Ok(Transaction {
            id: response.result.unwrap_or_default(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: code.to_string(),
            network: network.map(String::from),
            amount,
            status: TransactionStatus::Pending,
            address: Some(address.to_string()),
            tag: tag.map(String::from),
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_string());

        if let Some(n) = network {
            params.insert("chain".into(), n.to_string());
        }

        let response: XtDepositAddressResponse = self
            .private_request("GET", "/v4/deposit/address", params)
            .await?;

        if let Some(result) = response.result {
            Ok(DepositAddress {
                currency: code.to_string(),
                address: result.address.clone(),
                tag: result.address_memo.clone(),
                network: result.chain.clone(),
                info: serde_json::to_value(&result).unwrap_or_default(),
            })
        } else {
            Err(CcxtError::BadResponse { message: "Empty deposit address response".into() })
        }
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
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
        let url = format!("{}{}", Self::SPOT_URL, path);
        SignedRequest {
            url,
            method: method.to_string(),
            headers: HashMap::new(),
            body: if params.is_empty() {
                None
            } else {
                Some(serde_json::to_string(params).unwrap_or_default())
            },
        }
    }
}

// === XT API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XtResponse<T> {
    return_code: Option<i32>,
    msg_info: Option<String>,
    error: Option<serde_json::Value>,
    result: Option<T>,
}

type XtSymbolsResponse = XtResponse<Vec<XtSymbol>>;
type XtTickerResponse = XtResponse<Vec<XtTicker>>;
type XtOrderBookResponse = XtResponse<XtOrderBook>;
type XtTradesResponse = XtResponse<Vec<XtTrade>>;
type XtKlineResponse = XtResponse<Vec<Vec<String>>>;
type XtBalanceResponse = XtResponse<XtBalanceData>;
type XtOrderResponse = XtResponse<XtOrder>;
type XtOrdersResponse = XtResponse<Vec<XtOrder>>;
type XtMyTradesResponse = XtResponse<Vec<XtTrade>>;
type XtDepositsResponse = XtResponse<Vec<XtDeposit>>;
type XtWithdrawalsResponse = XtResponse<Vec<XtWithdrawal>>;
type XtWithdrawResponse = XtResponse<String>;
type XtDepositAddressResponse = XtResponse<XtDepositAddress>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XtSymbol {
    symbol: String,
    base_currency: String,
    quote_currency: String,
    #[serde(default)]
    base_precision: Option<i32>,
    #[serde(default)]
    quote_precision: Option<i32>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct XtTicker {
    s: Option<String>, // symbol
    t: Option<i64>,    // timestamp
    c: Option<String>, // close
    h: Option<String>, // high
    l: Option<String>, // low
    o: Option<String>, // open
    v: Option<String>, // volume
    q: Option<String>, // quote volume
    p: Option<String>, // price change percent
    a: Option<String>, // ask price
    b: Option<String>, // bid price
    #[serde(rename = "A")]
    av: Option<String>, // ask volume
    #[serde(rename = "B")]
    bv: Option<String>, // bid volume
    ch: Option<String>, // price change
}

#[derive(Debug, Deserialize)]
struct XtOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct XtTrade {
    i: Option<String>, // id
    t: Option<i64>,    // timestamp
    p: Option<String>, // price
    v: Option<String>, // volume
    s: Option<String>, // symbol
    m: Option<bool>,   // is buyer maker
}

#[derive(Debug, Deserialize)]
struct XtBalanceData {
    #[serde(default)]
    assets: Vec<XtBalance>,
}

#[derive(Debug, Deserialize)]
struct XtBalance {
    currency: String,
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    frozen: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XtOrder {
    #[serde(default)]
    order_id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    orig_qty: Option<String>,
    #[serde(default)]
    executed_qty: Option<String>,
    #[serde(default)]
    cum_quote: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    order_side: Option<String>,
    #[serde(default)]
    create_time: Option<i64>,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XtDeposit {
    #[serde(default)]
    id: Option<String>,
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_memo: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    updated_at: Option<i64>,
    #[serde(default)]
    confirmations: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XtWithdrawal {
    #[serde(default)]
    id: Option<String>,
    currency: String,
    #[serde(default)]
    chain: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    address_memo: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    updated_at: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XtDepositAddress {
    currency: String,
    address: String,
    #[serde(default)]
    address_memo: Option<String>,
    #[serde(default)]
    chain: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let xt = Xt::new(config).unwrap();

        assert_eq!(xt.to_market_id("BTC/USDT"), "btc_usdt");
        assert_eq!(xt.to_market_id("ETH/BTC"), "eth_btc");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let xt = Xt::new(config).unwrap();

        assert_eq!(xt.id(), ExchangeId::Xt);
        assert_eq!(xt.name(), "XT");
        assert!(xt.has().spot);
        assert!(xt.has().swap);
    }
}
