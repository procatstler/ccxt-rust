//! Coinmate Exchange Implementation
//!
//! Coinmate (GB, CZ, EU) - European cryptocurrency exchange

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Trade, Transaction,
    TransactionStatus, TransactionType, Fee, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Coinmate exchange
pub struct Coinmate {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<crate::types::Timeframe, String>,
}

impl Coinmate {
    const BASE_URL: &'static str = "https://coinmate.io/api";
    const RATE_LIMIT_MS: u64 = 600; // 100 requests per minute

    /// Create new Coinmate instance
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
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
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: false,
            fetch_withdrawals: false,
            withdraw: true,
            fetch_deposit_address: false,
            fetch_trading_fee: true,
            fetch_trading_fees: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87460806-1c9f3f00-c616-11ea-8c46-a77018a8f3f4.jpg".into()),
            api: api_urls,
            www: Some("https://coinmate.io".into()),
            doc: vec![
                "https://coinmate.docs.apiary.io".into(),
                "https://coinmate.io/developers".into(),
            ],
            fees: Some("https://coinmate.io/fees".into()),
        };

        Ok(Self {
            config,
            public_client,
            private_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes: HashMap::new(),
        })
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
            format!("{path}?{query}")
        } else {
            path.to_string()
        };

        self.public_client.get(&url, None, None).await
    }

    /// Private API call
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
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
        let uid = self.config.uid().ok_or_else(|| CcxtError::AuthenticationError {
            message: "UID required".into(),
        })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        // Create signature: nonce + uid + apiKey
        let auth = format!("{}{}{}", nonce, uid, api_key);

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(auth.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes()).to_uppercase();

        // Build request body
        let mut body_params = params.clone();
        body_params.insert("clientId".into(), uid.to_string());
        body_params.insert("nonce".into(), nonce);
        body_params.insert("publicKey".into(), api_key.to_string());
        body_params.insert("signature".into(), signature);

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());

        self.private_client.post_form(path, &body_params, Some(headers)).await
    }

    /// Parse ticker response
    fn parse_ticker(&self, data: &CoinmateTicker, market: &Market) -> Ticker {
        let timestamp = data.timestamp.map(|t| t * 1000);

        Ticker {
            symbol: market.symbol.clone(),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::<Utc>::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            high: data.high,
            low: data.low,
            bid: data.bid,
            bid_volume: None,
            ask: data.ask,
            ask_volume: None,
            vwap: None,
            open: data.open,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.change,
            percentage: None,
            average: None,
            base_volume: data.amount,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse order response
    fn parse_order(&self, data: &CoinmateOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELLED") => OrderStatus::Canceled,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("OPEN") => OrderStatus::Open,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_trade_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            _ => OrderType::Limit,
        };

        let side = match data.order_type.as_deref() {
            Some("BUY") => OrderSide::Buy,
            Some("SELL") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let amount = data.original_amount.as_ref()
            .or(data.amount.as_ref())
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();

        let remaining = data.remaining_amount.as_ref()
            .or(data.amount.as_ref())
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();

        Order {
            id: data.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp: data.timestamp,
            datetime: data.timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: data.price.as_ref().and_then(|p| p.parse().ok()),
            average: data.avg_price.as_ref().and_then(|p| p.parse().ok()),
            amount,
            filled: amount - remaining,
            remaining: Some(remaining),
            stop_price: data.stop_price.as_ref().and_then(|p| p.parse().ok()),
            trigger_price: None,
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

    /// Parse trade response
    fn parse_trade(&self, data: &CoinmateTrade, market: &Market) -> Trade {
        let timestamp = data.timestamp
            .or(data.created_timestamp)
            .map(|t| if t < 10000000000 { t * 1000 } else { t });

        let side = data.trade_type.as_ref()
            .or(data.order_type.as_ref())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let price: Decimal = data.price.as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();

        let amount: Decimal = data.amount.as_ref()
            .and_then(|a| a.parse().ok())
            .unwrap_or_default();

        let fee = data.fee.as_ref().and_then(|f| f.parse::<Decimal>().ok()).map(|cost| {
            Fee {
                cost: Some(cost),
                currency: Some(market.quote.clone()),
                rate: None,
            }
        });

        let taker_or_maker = data.fee_type.as_ref().map(|ft| {
            if ft == "MAKER" {
                TakerOrMaker::Maker
            } else {
                TakerOrMaker::Taker
            }
        });

        Trade {
            id: data.transaction_id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
            order: data.order_id.as_ref().map(|i| i.to_string()),
            timestamp,
            datetime: timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            symbol: market.symbol.clone(),
            trade_type: None,
            side: if !side.is_empty() { Some(side) } else { None },
            taker_or_maker,
            price,
            amount,
            cost: Some(price * amount),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Parse balance response
    fn parse_balance(&self, data: &HashMap<String, CoinmateBalance>) -> Balances {
        let mut result = Balances::new();

        for (currency, balance) in data {
            let free: Option<Decimal> = balance.available.as_ref()
                .and_then(|f| f.parse().ok());
            let used: Option<Decimal> = balance.reserved.as_ref()
                .and_then(|u| u.parse().ok());
            let total: Option<Decimal> = balance.balance.as_ref()
                .and_then(|t| t.parse().ok());

            let bal = Balance {
                free,
                used,
                total,
                debt: None,
            };
            result.add(currency, bal);
        }

        result
    }

    /// Parse transaction response
    fn parse_transaction(&self, data: &CoinmateTransaction) -> Transaction {
        let status = match data.transfer_status.as_deref() {
            Some("COMPLETED") | Some("OK") => TransactionStatus::Ok,
            Some("WAITING") | Some("SENT") | Some("CREATED") | Some("NEW") => TransactionStatus::Pending,
            Some("CANCELED") => TransactionStatus::Canceled,
            _ => TransactionStatus::Pending,
        };

        let tx_type = match data.transfer_type.as_deref() {
            Some("DEPOSIT") => TransactionType::Deposit,
            Some("WITHDRAWAL") => TransactionType::Withdrawal,
            _ => TransactionType::Deposit,
        };

        let fee = data.fee.as_ref().and_then(|f| f.parse::<Decimal>().ok()).map(|cost| {
            Fee {
                cost: Some(cost),
                currency: data.amount_currency.clone(),
                rate: None,
            }
        });

        Transaction {
            id: data.transaction_id
                .map(|i| i.to_string())
                .or_else(|| data.id.clone())
                .unwrap_or_default(),
            timestamp: data.timestamp,
            datetime: data.timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type,
            currency: data.amount_currency.clone().unwrap_or_default(),
            network: data.wallet_type.clone(),
            amount: data.amount.as_ref()
                .and_then(|a| a.parse().ok())
                .unwrap_or_default(),
            status,
            address: data.destination.clone(),
            tag: data.destination_tag.clone(),
            txid: data.txid.clone(),
            fee,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Coinmate {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinmate
    }

    fn name(&self) -> &str {
        "CoinMate"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["GB", "CZ", "EU"]
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

    fn timeframes(&self) -> &HashMap<crate::types::Timeframe, String> {
        // Coinmate doesn't support OHLCV
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
        let response: CoinmateApiResponse<Vec<CoinmateMarket>> = self
            .public_get("/tradingPairs", None)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No market data".into(),
        })?;

        let mut markets = Vec::new();

        for market_info in data {
            let base = market_info.first_currency.clone();
            let quote = market_info.second_currency.clone();
            let symbol = format!("{}/{}", base, quote);

            let price_precision = market_info.price_decimals.unwrap_or(2);
            let amount_precision = market_info.lot_decimals.unwrap_or(8);

            let market = Market {
                id: market_info.name.clone(),
                lowercase_id: Some(market_info.name.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: market_info.first_currency.clone(),
                quote_id: market_info.second_currency.clone(),
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
                taker: Some(Decimal::new(6, 3)), // 0.6%
                maker: Some(Decimal::new(4, 3)), // 0.4%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(amount_precision),
                    price: Some(price_precision),
                    cost: None,
                    base: Some(amount_precision),
                    quote: Some(price_precision),
                },
                limits: MarketLimits {
                    amount: MinMax {
                        min: market_info.min_amount,
                        max: None,
                    },
                    price: MinMax::default(),
                    cost: MinMax::default(),
                    leverage: MinMax::default(),
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_info).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let mut params = HashMap::new();
        params.insert("currencyPair".into(), market.id.clone());

        let response: CoinmateApiResponse<CoinmateTicker> = self
            .public_get("/ticker", Some(params))
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data".into(),
        })?;

        Ok(self.parse_ticker(&data, market))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.load_markets(false).await?;

        let response: CoinmateApiResponse<HashMap<String, CoinmateTicker>> = self
            .public_get("/tickerAll", None)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data".into(),
        })?;

        let markets = self.markets.read().unwrap().clone();
        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut tickers = HashMap::new();

        for (market_id, ticker_data) in data {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                if let Some(market) = markets.get(symbol) {
                    let ticker = self.parse_ticker(&ticker_data, market);
                    tickers.insert(symbol.clone(), ticker);
                }
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let mut params = HashMap::new();
        params.insert("currencyPair".into(), market.id.clone());
        params.insert("groupByPriceLimit".into(), "False".into());

        let response: CoinmateApiResponse<CoinmateOrderBook> = self
            .public_get("/orderBook", Some(params))
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order book data".into(),
        })?;

        let mut bids: Vec<OrderBookEntry> = data.bids.iter().map(|b| {
            OrderBookEntry {
                price: b.price.unwrap_or_default(),
                amount: b.amount.unwrap_or_default(),
            }
        }).collect();

        let mut asks: Vec<OrderBookEntry> = data.asks.iter().map(|a| {
            OrderBookEntry {
                price: a.price.unwrap_or_default(),
                amount: a.amount.unwrap_or_default(),
            }
        }).collect();

        // Apply limit if specified
        if let Some(l) = limit {
            let limit = l as usize;
            bids.truncate(limit);
            asks.truncate(limit);
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: data.timestamp,
            datetime: data.timestamp.and_then(|t| {
                chrono::DateTime::from_timestamp_millis(t)
                    .map(|dt| dt.to_rfc3339())
            }),
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

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let mut params = HashMap::new();
        params.insert("currencyPair".into(), market.id.clone());
        params.insert("minutesIntoHistory".into(), "10".into());

        let response: CoinmateApiResponse<Vec<CoinmateTrade>> = self
            .public_get("/transactions", Some(params))
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No trade data".into(),
        })?;

        let mut trades: Vec<Trade> = data.iter()
            .map(|t| self.parse_trade(t, market))
            .collect();

        if let Some(l) = limit {
            trades.truncate(l as usize);
        }

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        _symbol: &str,
        _timeframe: crate::types::Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv".into(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: CoinmateApiResponse<HashMap<String, CoinmateBalance>> = self
            .private_post("/balances", HashMap::new())
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No balance data".into(),
        })?;

        Ok(self.parse_balance(&data))
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

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let mut params = HashMap::new();
        params.insert("currencyPair".into(), market.id.clone());

        let method = match (side, order_type) {
            (OrderSide::Buy, OrderType::Limit) => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("amount".into(), amount.to_string());
                params.insert("price".into(), price_val.to_string());
                "/buyLimit"
            }
            (OrderSide::Sell, OrderType::Limit) => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("amount".into(), amount.to_string());
                params.insert("price".into(), price_val.to_string());
                "/sellLimit"
            }
            (OrderSide::Buy, OrderType::Market) => {
                params.insert("total".into(), amount.to_string());
                "/buyInstant"
            }
            (OrderSide::Sell, OrderType::Market) => {
                params.insert("amount".into(), amount.to_string());
                "/sellInstant"
            }
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {:?}", order_type),
                });
            }
        };

        let response: CoinmateApiResponse<String> = self
            .private_post(method, params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let order_id = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order ID returned".into(),
        })?;

        Ok(Order {
            id: order_id,
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

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let response: CoinmateApiResponse<CoinmateCancelResponse> = self
            .private_post("/cancelOrderWithInfo", params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No cancel data".into(),
        })?;

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: String::new(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side: OrderSide::Buy,
            price: None,
            average: None,
            amount: Decimal::ZERO,
            filled: Decimal::ZERO,
            remaining: data.remaining_amount.as_ref()
                .and_then(|a| a.parse().ok()),
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
            info: serde_json::to_value(&data).unwrap_or_default(),
        })
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderId".into(), id.to_string());

        let response: CoinmateApiResponse<CoinmateOrder> = self
            .private_post("/orderById", params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order data".into(),
        })?;

        // Determine symbol from currency pair
        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let symbol = data.currency_pair.as_ref()
            .and_then(|id| markets_by_id.get(id))
            .map(|s| s.as_str())
            .unwrap_or("");

        Ok(self.parse_order(&data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap().clone();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.into(),
            })?;
            params.insert("currencyPair".into(), market.id.clone());
        }

        let response: CoinmateApiResponse<Vec<CoinmateOrder>> = self
            .private_post("/openOrders", params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order data".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = data.iter().map(|o| {
            let sym = o.currency_pair.as_ref()
                .and_then(|id| markets_by_id.get(id))
                .map(|s| s.as_str())
                .unwrap_or("");
            self.parse_order(o, sym)
        }).collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.load_markets(false).await?;

        let mut params = HashMap::new();
        params.insert("limit".into(), limit.unwrap_or(1000).to_string());

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap().clone();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.into(),
            })?;
            params.insert("currencyPair".into(), market.id.clone());
        }

        if let Some(ts) = since {
            params.insert("timestampFrom".into(), ts.to_string());
        }

        let response: CoinmateApiResponse<Vec<CoinmateTrade>> = self
            .private_post("/tradeHistory", params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No trade data".into(),
        })?;

        let markets = self.markets.read().unwrap().clone();
        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let trades: Vec<Trade> = data.iter().filter_map(|t| {
            let market_id = t.currency_pair.as_ref()?;
            let symbol = markets_by_id.get(market_id)?;
            let market = markets.get(symbol)?;
            Some(self.parse_trade(t, market))
        }).collect();

        Ok(trades)
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
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("destinationTag".into(), t.to_string());
        }

        // Determine withdrawal method based on currency
        let method = match code {
            "BTC" => "/bitcoinWithdrawal",
            "LTC" => "/litecoinWithdrawal",
            "BCH" => "/bitcoinCashWithdrawal",
            "ETH" => "/ethereumWithdrawal",
            "XRP" => "/rippleWithdrawal",
            "DASH" => "/dashWithdrawal",
            "ADA" => "/adaWithdrawal",
            "SOL" => "/solWithdrawal",
            _ => {
                params.insert("currency".into(), code.to_string());
                "/withdrawVirtualCurrency"
            }
        };

        let response: CoinmateApiResponse<CoinmateWithdrawResponse> = self
            .private_post(method, params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No withdrawal data".into(),
        })?;

        Ok(Transaction {
            id: data.id,
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

    async fn fetch_trading_fee(&self, symbol: &str) -> CcxtResult<crate::types::TradingFee> {
        self.load_markets(false).await?;

        let markets = self.markets.read().unwrap().clone();
        let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
            symbol: symbol.into(),
        })?;

        let mut params = HashMap::new();
        params.insert("currencyPair".into(), market.id.clone());

        let response: CoinmateApiResponse<CoinmateFee> = self
            .private_post("/traderFees", params)
            .await?;

        if response.error {
            return Err(CcxtError::ExchangeError {
                message: response.error_message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No fee data".into(),
        })?;

        let maker = data.maker.as_ref()
            .and_then(|m| m.parse::<Decimal>().ok())
            .map(|m| m / Decimal::from(100))
            .unwrap_or_default();

        let taker = data.taker.as_ref()
            .and_then(|t| t.parse::<Decimal>().ok())
            .map(|t| t / Decimal::from(100))
            .unwrap_or_default();

        Ok(crate::types::TradingFee::new(symbol, maker, taker))
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
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();
        let mut body = None;

        if api == "public" {
            if !params.is_empty() {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                url = format!("{url}?{query}");
            }
        } else {
            // Private API
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();
            let uid = self.config.uid().unwrap_or_default();
            let nonce = Utc::now().timestamp_millis().to_string();

            let auth = format!("{}{}{}", nonce, uid, api_key);
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(auth.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes()).to_uppercase();

            let mut body_params = params.clone();
            body_params.insert("clientId".into(), uid.to_string());
            body_params.insert("nonce".into(), nonce);
            body_params.insert("publicKey".into(), api_key.to_string());
            body_params.insert("signature".into(), signature);

            let body_str: String = body_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());
            body = Some(body_str);
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body,
        }
    }
}

// === Coinmate API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateApiResponse<T> {
    error: bool,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateMarket {
    name: String,
    first_currency: String,
    second_currency: String,
    #[serde(default)]
    price_decimals: Option<i32>,
    #[serde(default)]
    lot_decimals: Option<i32>,
    #[serde(default)]
    min_amount: Option<Decimal>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateTicker {
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    change: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    bids: Vec<CoinmateOrderBookLevel>,
    asks: Vec<CoinmateOrderBookLevel>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct CoinmateOrderBookLevel {
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateTrade {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    transaction_id: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    currency_pair: Option<String>,
    #[serde(default)]
    trade_type: Option<String>,
    // For private trades
    #[serde(default)]
    created_timestamp: Option<i64>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    fee_type: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateOrder {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    currency_pair: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    remaining_amount: Option<String>,
    #[serde(default)]
    original_amount: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    order_trade_type: Option<String>,
    #[serde(default)]
    avg_price: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateBalance {
    #[serde(default)]
    available: Option<String>,
    #[serde(default)]
    reserved: Option<String>,
    #[serde(default)]
    balance: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateTransaction {
    #[serde(default)]
    transaction_id: Option<i64>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    amount_currency: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    wallet_type: Option<String>,
    #[serde(default)]
    transfer_type: Option<String>,
    #[serde(default)]
    transfer_status: Option<String>,
    #[serde(default)]
    txid: Option<String>,
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    destination_tag: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoinmateCancelResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    remaining_amount: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct CoinmateWithdrawResponse {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct CoinmateFee {
    #[serde(default)]
    maker: Option<String>,
    #[serde(default)]
    taker: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let coinmate = Coinmate::new(config).unwrap();

        assert_eq!(coinmate.id(), ExchangeId::Coinmate);
        assert_eq!(coinmate.name(), "CoinMate");
        assert!(coinmate.has().spot);
        assert!(!coinmate.has().margin);
        assert!(!coinmate.has().swap);
    }
}
