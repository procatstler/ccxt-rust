//! Korbit Exchange Implementation
//!
//! CCXT korbit.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::RwLock as TokioRwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Transaction,
    TransactionStatus, TransactionType, WsExchange, WsMessage, OHLCV,
};

use super::korbit_ws::KorbitWs;

/// Korbit 거래소
pub struct Korbit {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    access_token: RwLock<Option<String>>,
    token_expiry: RwLock<i64>,
    ws_client: Arc<TokioRwLock<KorbitWs>>,
}

impl Korbit {
    const API_URL: &'static str = "https://api.korbit.co.kr";
    const RATE_LIMIT_MS: u64 = 500; // 초당 2회

    /// 새 Korbit 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::API_URL, &config)?;
        let private_client = HttpClient::new(Self::API_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            fetch_markets: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false, // Korbit doesn't have public OHLCV endpoint
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            fetch_deposit_address: true,
            withdraw: true,
            ws: true,
            watch_ticker: true,
            watch_order_book: true,
            watch_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::API_URL.into());
        api_urls.insert("private".into(), Self::API_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766815-bcdf9d66-5ef0-11e7-9aa8-e1b2b9c2cc0d.jpg".into()),
            api: api_urls,
            www: Some("https://www.korbit.co.kr".into()),
            doc: vec!["https://apidocs.korbit.co.kr".into()],
            fees: Some("https://www.korbit.co.kr/about/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1".into());
        timeframes.insert(Timeframe::Minute3, "3".into());
        timeframes.insert(Timeframe::Minute5, "5".into());
        timeframes.insert(Timeframe::Minute15, "15".into());
        timeframes.insert(Timeframe::Minute30, "30".into());
        timeframes.insert(Timeframe::Hour1, "60".into());
        timeframes.insert(Timeframe::Hour4, "240".into());
        timeframes.insert(Timeframe::Day1, "1440".into());

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
            access_token: RwLock::new(None),
            token_expiry: RwLock::new(0),
            ws_client: Arc::new(TokioRwLock::new(KorbitWs::new())),
        })
    }

    /// OAuth2 토큰 획득/갱신
    async fn get_access_token(&self) -> CcxtResult<String> {
        // Check if we have a valid token
        {
            let token = self.access_token.read().unwrap();
            let expiry = *self.token_expiry.read().unwrap();
            let now = Utc::now().timestamp();

            if token.is_some() && expiry > now + 60 {
                return Ok(token.clone().unwrap());
            }
        }

        // Need to get new token
        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let mut params = HashMap::new();
        params.insert("client_id".into(), api_key.to_string());
        params.insert("client_secret".into(), secret.to_string());
        params.insert("grant_type".into(), "client_credentials".into());

        let response: KorbitTokenResponse = self
            .public_client
            .post_form("/v1/oauth2/access_token", &params, None)
            .await?;

        let token = response.access_token;
        let expires_in = response.expires_in.unwrap_or(3600);
        let expiry = Utc::now().timestamp() + expires_in;

        {
            let mut token_guard = self.access_token.write().unwrap();
            *token_guard = Some(token.clone());
        }
        {
            let mut expiry_guard = self.token_expiry.write().unwrap();
            *expiry_guard = expiry;
        }

        Ok(token)
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

    /// 비공개 API 호출 (GET)
    async fn private_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let token = self.get_access_token().await?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {token}"));

        self.private_client.get(path, params, Some(headers)).await
    }

    /// 비공개 API 호출 (POST)
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        let token = self.get_access_token().await?;

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), format!("Bearer {token}"));
        headers.insert(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        );

        self.private_client
            .post_form(path, params, Some(headers))
            .await
    }

    /// 심볼 → 마켓 ID 변환 (BTC/KRW → btc_krw)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }

    /// 마켓 ID → 심볼 변환 (btc_krw → BTC/KRW)
    fn to_symbol(&self, market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('_').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
        } else {
            market_id.to_uppercase()
        }
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &KorbitTickerData, symbol: &str) -> Ticker {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|s| s.parse().ok()),
            low: data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: data.bid.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: data.ask.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: data.open.as_ref().and_then(|s| s.parse().ok()),
            close: data.last.as_ref().and_then(|s| s.parse().ok()),
            last: data.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: data.change.as_ref().and_then(|s| s.parse().ok()),
            percentage: data.change_percent.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.volume.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        }
    }

    /// 호가창 응답 파싱
    fn parse_order_book(&self, data: &KorbitOrderBookData, symbol: &str) -> OrderBook {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                if b.len() >= 2 {
                    Some(OrderBookEntry::new(b[0].parse().ok()?, b[1].parse().ok()?))
                } else {
                    None
                }
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                if a.len() >= 2 {
                    Some(OrderBookEntry::new(a[0].parse().ok()?, a[1].parse().ok()?))
                } else {
                    None
                }
            })
            .collect();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            bids,
            asks,
            checksum: None,
            nonce: None,
        }
    }

    /// 체결 내역 파싱
    fn parse_trade(&self, data: &KorbitTradeData, symbol: &str) -> Trade {
        let timestamp = data
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .amount
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        Trade {
            id: data.tid.map(|id| id.to_string()).unwrap_or_default(),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.r#type.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, data: &HashMap<String, KorbitBalanceData>) -> Balances {
        let mut balances = Balances::new();

        for (currency, balance_data) in data {
            let total: Option<Decimal> = balance_data.total.as_ref().and_then(|s| s.parse().ok());
            let free: Option<Decimal> =
                balance_data.available.as_ref().and_then(|s| s.parse().ok());
            let used: Option<Decimal> = balance_data
                .trade_in_use
                .as_ref()
                .and_then(|s| s.parse().ok());

            let balance = Balance {
                free,
                used,
                total,
                debt: None,
            };
            balances.add(currency.to_uppercase(), balance);
        }

        balances
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "unfilled" => OrderStatus::Open,
            "partially_filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &KorbitOrderData) -> Order {
        let status = data
            .status
            .as_ref()
            .map(|s| self.parse_order_status(s))
            .unwrap_or(OrderStatus::Open);

        let order_type = match data.order_type.as_deref() {
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.r#type.as_deref() {
            Some("bid") | Some("buy") => OrderSide::Buy,
            Some("ask") | Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let symbol = data
            .currency_pair
            .as_ref()
            .map(|cp| self.to_symbol(cp))
            .unwrap_or_default();

        let price: Option<Decimal> = data.price.as_ref().and_then(|s| s.parse().ok());
        let amount: Decimal = data
            .total
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .filled_total
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let remaining = Some(amount - filled);

        let timestamp = data.created_at;

        Order {
            id: data.id.map(|id| id.to_string()).unwrap_or_default(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
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
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::Value::Null,
        }
    }

    /// 거래 내역 파싱
    fn parse_user_trade(&self, data: &KorbitUserTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp;
        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .amount
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|s| s.parse().ok());

        Trade {
            id: data.id.map(|id| id.to_string()).unwrap_or_default(),
            order: data.order_id.map(|id| id.to_string()),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: data.r#type.clone(),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: fee_amount.map(|f| Fee::new(f, "KRW".to_string())),
            fees: Vec::new(),
            info: serde_json::Value::Null,
        }
    }

    /// 입출금 내역 파싱
    fn parse_transaction(&self, data: &KorbitTransaction, tx_type: TransactionType) -> Transaction {
        let timestamp = data.created_at;
        let amount: Decimal = data
            .amount
            .as_ref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|s| s.parse().ok());

        let status = match data.status.as_deref() {
            Some("pending") => TransactionStatus::Pending,
            Some("completed") | Some("success") => TransactionStatus::Ok,
            Some("failed") | Some("canceled") => TransactionStatus::Canceled,
            _ => TransactionStatus::Pending,
        };

        Transaction {
            id: data.id.map(|id| id.to_string()).unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type,
            currency: data.currency.clone().unwrap_or_default().to_uppercase(),
            network: None,
            amount,
            status,
            address: data.address.clone(),
            tag: data.destination_tag.clone(),
            txid: data.txid.clone(),
            fee: fee_amount
                .map(|f| Fee::new(f, data.currency.clone().unwrap_or_default().to_uppercase())),
            internal: None,
            confirmations: data.confirmations.map(|c| c as u32),
            info: serde_json::Value::Null,
        }
    }
}

#[async_trait]
impl Exchange for Korbit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Korbit
    }

    fn name(&self) -> &str {
        "Korbit"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["KR"]
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
        let response: KorbitTickersResponse =
            self.public_get("/v1/ticker/detailed/all", None).await?;

        let mut markets = Vec::new();
        for (market_id, _) in response {
            if market_id == "timestamp" {
                continue;
            }

            let parts: Vec<&str> = market_id.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base = parts[0].to_uppercase();
            let quote = parts[1].to_uppercase();
            let symbol = format!("{base}/{quote}");

            markets.push(Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                settle: None,
                base_id: parts[0].to_string(),
                quote_id: parts[1].to_string(),
                settle_id: None,
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
                taker: Some(Decimal::new(15, 4)), // 0.15%
                maker: Some(Decimal::new(15, 4)), // 0.15%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision::default(),
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::Value::Null,
                tier_based: true,
                percentage: true,
            });
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);

        let response: KorbitTickerData =
            self.public_get("/v1/ticker/detailed", Some(params)).await?;
        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let response: KorbitTickersResponse =
            self.public_get("/v1/ticker/detailed/all", None).await?;

        let mut tickers = HashMap::new();
        for (market_id, ticker_data) in response {
            if market_id == "timestamp" {
                continue;
            }
            let symbol = self.to_symbol(&market_id);
            let ticker = self.parse_ticker(&ticker_data, &symbol);
            tickers.insert(symbol, ticker);
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);

        let response: KorbitOrderBookData = self.public_get("/v1/orderbook", Some(params)).await?;
        Ok(self.parse_order_book(&response, symbol))
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("currency_pair".into(), market_id);

        if let Some(_ts) = since {
            params.insert("time".into(), "minute".into());
        }

        let response: Vec<KorbitTradeData> =
            self.public_get("/v1/transactions", Some(params)).await?;

        let mut trades: Vec<Trade> = response
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect();

        if let Some(l) = limit {
            trades.truncate(l as usize);
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        // Korbit doesn't provide public OHLCV endpoint
        Err(CcxtError::NotSupported {
            feature: "fetchOhlcv".into(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: HashMap<String, KorbitBalanceData> =
            self.private_get("/v1/user/balances", None).await?;
        Ok(self.parse_balance(&response))
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
        params.insert("currency_pair".into(), market_id);
        params.insert(
            "type".into(),
            match side {
                OrderSide::Buy => "bid",
                OrderSide::Sell => "ask",
            }
            .into(),
        );

        match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("price".into(), price_val.to_string());
                params.insert("coin_amount".into(), amount.to_string());
            },
            OrderType::Market => {
                match side {
                    OrderSide::Buy => {
                        // For market buy, specify fiat amount
                        params.insert("fiat_amount".into(), amount.to_string());
                    },
                    OrderSide::Sell => {
                        params.insert("coin_amount".into(), amount.to_string());
                    },
                }
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                });
            },
        }

        let endpoint = match side {
            OrderSide::Buy => "/v1/user/orders/buy",
            OrderSide::Sell => "/v1/user/orders/sell",
        };
        let response: KorbitOrderResponse = self.private_post(endpoint, &params).await?;

        Ok(Order {
            id: response
                .order_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            client_order_id: None,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::Value::Null,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());
        params.insert("currency_pair".into(), market_id);

        let _response: serde_json::Value =
            self.private_post("/v1/user/orders/cancel", &params).await?;

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
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());
        params.insert("currency_pair".into(), market_id);

        let response: KorbitOrderData = self.private_get("/v1/user/orders", Some(params)).await?;
        Ok(self.parse_order(&response))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let market_id = symbol.map(|s| self.to_market_id(s));

        let mut params = HashMap::new();
        if let Some(mid) = market_id {
            params.insert("currency_pair".into(), mid);
        }

        let response: Vec<KorbitOrderData> = self
            .private_get(
                "/v1/user/orders/open",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        Ok(response.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let market_id = symbol.map(|s| self.to_market_id(s));

        let mut params = HashMap::new();
        if let Some(mid) = market_id {
            params.insert("currency_pair".into(), mid);
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<KorbitOrderData> = self
            .private_get(
                "/v1/user/orders",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        Ok(response
            .iter()
            .filter(|o| {
                o.status.as_deref() == Some("filled") || o.status.as_deref() == Some("canceled")
            })
            .map(|o| self.parse_order(o))
            .collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = symbol.map(|s| self.to_market_id(s));

        let mut params = HashMap::new();
        if let Some(mid) = &market_id {
            params.insert("currency_pair".into(), mid.clone());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: Vec<KorbitUserTrade> = self
            .private_get(
                "/v1/user/transactions",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        let symbol_str = symbol.unwrap_or("BTC/KRW");
        Ok(response
            .iter()
            .filter(|t| t.r#type.as_deref() == Some("buy") || t.r#type.as_deref() == Some("sell"))
            .map(|t| self.parse_user_trade(t, symbol_str))
            .collect())
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_lowercase());
        }

        let response: Vec<KorbitTransaction> = self
            .private_get(
                "/v1/user/deposits",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        Ok(response
            .iter()
            .map(|t| self.parse_transaction(t, TransactionType::Deposit))
            .collect())
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_lowercase());
        }

        let response: Vec<KorbitTransaction> = self
            .private_get(
                "/v1/user/withdrawals",
                if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            )
            .await?;

        Ok(response
            .iter()
            .map(|t| self.parse_transaction(t, TransactionType::Withdrawal))
            .collect())
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_lowercase());

        let response: KorbitDepositAddress = self
            .private_get("/v1/user/wallet_address", Some(params))
            .await?;

        Ok(DepositAddress {
            currency: code.to_string(),
            address: response.address.unwrap_or_default(),
            tag: response.destination_tag,
            network: None,
            info: serde_json::Value::Null,
        })
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
        params.insert("currency".into(), code.to_lowercase());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("destination_tag".into(), t.to_string());
        }

        let response: KorbitWithdrawResponse = self
            .private_post("/v1/user/withdrawals/coin", &params)
            .await?;

        Ok(Transaction {
            id: response
                .transfer_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: code.to_string(),
            network: None,
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

    fn market_id(&self, symbol: &str) -> Option<String> {
        Some(self.to_market_id(symbol))
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        Some(self.to_symbol(market_id))
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
        let url = format!("{}{}", Self::API_URL, path);
        let headers = HashMap::new();

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    // === WebSocket Methods ===

    async fn watch_ticker(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ticker(symbol).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_tickers(symbols).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_order_book(symbol, limit).await
    }

    async fn watch_trades(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_trades(symbol).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ohlcv(symbol, timeframe).await
    }
}

// === Korbit API Response Types ===

#[derive(Debug, Deserialize)]
struct KorbitTokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    token_type: Option<String>,
}

type KorbitTickersResponse = HashMap<String, KorbitTickerData>;

#[derive(Debug, Default, Deserialize, Serialize)]
struct KorbitTickerData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    bid: Option<String>,
    #[serde(default)]
    ask: Option<String>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    change: Option<String>,
    #[serde(default, rename = "changePercent")]
    change_percent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitOrderBookData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitTradeData {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    tid: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default, rename = "type")]
    r#type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitBalanceData {
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    trade_in_use: Option<String>,
    #[serde(default)]
    withdrawal_in_use: Option<String>,
    #[serde(default)]
    total: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitOrderData {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    currency_pair: Option<String>,
    #[serde(default, rename = "type")]
    r#type: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    filled_total: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    created_at: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitOrderResponse {
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitUserTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default, rename = "type")]
    r#type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitTransaction {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    created_at: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    destination_tag: Option<String>,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    confirmations: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitDepositAddress {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    destination_tag: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct KorbitWithdrawResponse {
    #[serde(default)]
    transfer_id: Option<i64>,
    #[serde(default)]
    status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let korbit = Korbit::new(config).unwrap();

        assert_eq!(korbit.to_market_id("BTC/KRW"), "btc_krw");
        assert_eq!(korbit.to_symbol("btc_krw"), "BTC/KRW");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let korbit = Korbit::new(config).unwrap();

        assert_eq!(korbit.id(), ExchangeId::Korbit);
        assert_eq!(korbit.name(), "Korbit");
        assert!(korbit.has().spot);
        assert!(!korbit.has().swap);
    }
}
