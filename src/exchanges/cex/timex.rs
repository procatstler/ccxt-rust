//! Timex Exchange Implementation
//!
//! CCXT timex.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Currency, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade,
    Transaction, TransactionStatus, TransactionType, Fee, OHLCV,
};

/// Timex 거래소
pub struct Timex {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    currencies: RwLock<HashMap<String, Currency>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Timex {
    const BASE_URL: &'static str = "https://plasma-relay-backend.timex.io";
    const RATE_LIMIT_MS: u64 = 1500;

    /// 새 Timex 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: false,
            spot: true,
            margin: false,
            swap: false,
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
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            fetch_deposit_address: true,
            fetch_time: true,
            fetch_trading_fee: true,
            edit_order: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("rest".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/70423869-6839ab00-1a7f-11ea-8f94-13ae72c31115.jpg".into()),
            api: api_urls,
            www: Some("https://timex.io".into()),
            doc: vec!["https://plasma-relay-backend.timex.io/swagger-ui/index.html".into()],
            fees: None,
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "I1".into());
        timeframes.insert(Timeframe::Minute5, "I5".into());
        timeframes.insert(Timeframe::Minute15, "I15".into());
        timeframes.insert(Timeframe::Minute30, "I30".into());
        timeframes.insert(Timeframe::Hour1, "H1".into());
        timeframes.insert(Timeframe::Hour2, "H2".into());
        timeframes.insert(Timeframe::Hour4, "H4".into());
        timeframes.insert(Timeframe::Hour6, "H6".into());
        timeframes.insert(Timeframe::Hour12, "H12".into());
        timeframes.insert(Timeframe::Day1, "D1".into());
        timeframes.insert(Timeframe::Week1, "W1".into());

        Ok(Self {
            config,
            public_client,
            private_client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            currencies: RwLock::new(HashMap::new()),
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

        self.public_client.get(&url, None, None).await
    }

    /// 비공개 API 호출 - Basic Auth 사용
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

        // Timex uses Basic Auth
        let auth_string = format!("{api_key}:{secret}");
        let encoded = BASE64.encode(auth_string);
        let auth_header = format!("Basic {encoded}");

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), auth_header);

        match method {
            "GET" | "DELETE" => {
                // For GET and DELETE, use query parameters in URL
                let url = if params.is_empty() {
                    path.to_string()
                } else {
                    let query: String = params
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                        .collect::<Vec<_>>()
                        .join("&");
                    format!("{path}?{query}")
                };

                if method == "GET" {
                    self.private_client.get(&url, None, Some(headers)).await
                } else {
                    self.private_client.delete(&url, None, Some(headers)).await
                }
            }
            "POST" => {
                // For POST, send params as JSON body
                let body = if params.is_empty() {
                    None
                } else {
                    let json_params: serde_json::Value = params
                        .into_iter()
                        .map(|(k, v)| (k, serde_json::Value::String(v)))
                        .collect();
                    Some(json_params)
                };
                self.private_client.post(path, body, Some(headers)).await
            }
            "PUT" => {
                // For PUT, pass params HashMap directly (not as query string)
                self.private_client.put(path, params, Some(headers)).await
            }
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &TimexTicker, symbol: &str) -> Ticker {
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime: data.timestamp.clone(),
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
            change: None,
            percentage: None,
            average: None,
            base_volume: data.volume,
            quote_volume: data.volume_quote,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &TimexOrder, symbol: &str) -> Order {
        let status = match data.status.as_deref() {
            None => {
                // Determine status from quantity
                let quantity = data.quantity.unwrap_or_default();
                let filled = data.filled_quantity.unwrap_or_default();
                let cancelled = data.cancelled_quantity.unwrap_or_default();

                if filled == quantity {
                    OrderStatus::Closed
                } else if cancelled > Decimal::ZERO {
                    OrderStatus::Canceled
                } else {
                    OrderStatus::Open
                }
            }
            Some("NEW") => OrderStatus::Open,
            Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FILLED") => OrderStatus::Closed,
            Some("CANCELED") => OrderStatus::Canceled,
            Some("PENDING_CANCEL") => OrderStatus::Canceled,
            Some("REJECTED") => OrderStatus::Rejected,
            Some("EXPIRED") => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_deref() {
            Some("LIMIT") => OrderType::Limit,
            Some("MARKET") => OrderType::Market,
            Some("POST_ONLY") => OrderType::Limit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let timestamp = data.created_at
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let amount = data.quantity.unwrap_or_default();
        let filled = data.filled_quantity.unwrap_or_default();
        let remaining = Some(amount - filled);

        Order {
            id: data.id.clone().unwrap_or_default(),
            client_order_id: data.client_order_id.clone(),
            timestamp,
            datetime: data.created_at.clone(),
            last_trade_timestamp: data.updated_at
                .as_ref()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|dt| dt.timestamp_millis()),
            last_update_timestamp: data.updated_at
                .as_ref()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|dt| dt.timestamp_millis()),
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: data.price,
            average: None,
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
            post_only: Some(data.order_type.as_deref() == Some("POST_ONLY")),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &[TimexBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let total = b.total_balance;
            let used = b.locked_balance;
            let free = total.map(|t| {
                let u = used.unwrap_or_default();
                t - u
            });

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

    /// 거래 내역 파싱
    fn parse_trade(&self, data: &TimexTrade, symbol: &str) -> Trade {
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let side = match data.side.as_deref().or(data.direction.as_deref()) {
            Some("BUY") => Some("buy".into()),
            Some("SELL") => Some("sell".into()),
            _ => None,
        };

        let taker_or_maker = data.maker_or_taker.as_ref().and_then(|m| match m.as_str() {
            "MAKER" => Some(TakerOrMaker::Maker),
            "TAKER" => Some(TakerOrMaker::Taker),
            _ => None,
        });

        let price = data.price.unwrap_or_default();
        let amount = data.quantity.unwrap_or_default();
        let cost = Some(price * amount);

        let fee = data.fee.map(|fee_cost| Fee {
            cost: Some(fee_cost),
            currency: None,
            rate: None,
        });

        Trade {
            id: data.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
            order: data.maker_order_id.clone().or(data.taker_order_id.clone()),
            timestamp,
            datetime: data.timestamp.clone(),
            symbol: symbol.to_string(),
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost,
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// Timeframe을 초 단위로 변환
    fn parse_timeframe(&self, timeframe: Timeframe) -> CcxtResult<i64> {
        Ok(match timeframe {
            Timeframe::Minute1 => 60,
            Timeframe::Minute5 => 300,
            Timeframe::Minute15 => 900,
            Timeframe::Minute30 => 1800,
            Timeframe::Hour1 => 3600,
            Timeframe::Hour2 => 7200,
            Timeframe::Hour4 => 14400,
            Timeframe::Hour6 => 21600,
            Timeframe::Hour12 => 43200,
            Timeframe::Day1 => 86400,
            Timeframe::Week1 => 604800,
            _ => return Err(CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            }),
        })
    }

    /// 입출금 내역 파싱
    fn parse_transaction(&self, data: &TimexTransaction) -> Transaction {
        let timestamp = data.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        Transaction {
            id: data.transfer_hash.clone().unwrap_or_default(),
            timestamp,
            datetime: data.timestamp.clone(),
            updated: None,
            tx_type: TransactionType::Deposit, // Will be set by caller
            currency: String::new(), // Will be set by caller
            network: None,
            amount: data.value.unwrap_or_default(),
            status: TransactionStatus::Ok,
            address: data.to.clone(),
            tag: None,
            txid: data.transfer_hash.clone(),
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Timex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Timex
    }

    fn name(&self) -> &str {
        "TimeX"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["AU"]
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
        let response: Vec<TimexMarket> = self
            .public_get("/public/markets", None)
            .await?;

        let mut markets = Vec::new();

        for market_info in response {
            if market_info.locked.unwrap_or(false) {
                continue;
            }

            let base = market_info.base_currency.clone();
            let quote = market_info.quote_currency.clone();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: market_info.symbol.clone(),
                lowercase_id: Some(market_info.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: market_info.base_currency.clone(),
                quote_id: market_info.quote_currency.clone(),
                settle: None,
                settle_id: None,
                active: !market_info.locked.unwrap_or(false),
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
                taker: market_info.taker_fee,
                maker: market_info.maker_fee,
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: None,
                    price: None,
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits {
                    leverage: MinMax { min: None, max: None },
                    amount: crate::types::MinMax {
                        min: market_info.base_min_size,
                        max: None,
                    },
                    price: crate::types::MinMax {
                        min: market_info.tick_size,
                        max: None,
                    },
                    cost: crate::types::MinMax {
                        min: market_info.quote_min_size,
                        max: None,
                    },
                },
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&market_info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("period".into(), "D1".into());

        let response: Vec<TimexTicker> = self
            .public_get("/public/tickers", Some(params))
            .await?;

        let ticker_data = response.first().ok_or_else(|| CcxtError::ExchangeError {
            message: format!("No ticker data for {symbol}"),
        })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let mut params = HashMap::new();
        params.insert("period".into(), "D1".into());

        let response: Vec<TimexTicker> = self
            .public_get("/public/tickers", Some(params))
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();
        let mut tickers = HashMap::new();

        for data in response {
            if let Some(market_name) = &data.market {
                if let Some(symbol) = markets_by_id.get(market_name) {
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

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: TimexOrderBook = self
            .public_get("/public/orderbook/v2", Some(params))
            .await?;

        let timestamp = response.timestamp
            .as_ref()
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis());

        let bids: Vec<OrderBookEntry> = response
            .bid
            .iter()
            .map(|b| OrderBookEntry {
                price: b.price.unwrap_or_default(),
                amount: b.base_token_amount.unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .ask
            .iter()
            .map(|a| OrderBookEntry {
                price: a.price.unwrap_or_default(),
                amount: a.base_token_amount.unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp,
            datetime: response.timestamp,
            nonce: None,
            bids,
            asks,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("sort".into(), "timestamp,asc".into());

        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?;
            params.insert("from".into(), datetime.to_rfc3339());
        }
        if let Some(l) = limit {
            params.insert("size".into(), l.to_string());
        }

        let response: Vec<TimexTrade> = self
            .public_get("/public/trades", Some(params))
            .await?;

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
        let (market_id, period) = {
            let markets = self.markets.read().unwrap();
            let market = markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?;
            let period = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;
            (market.id.clone(), period.clone())
        };

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("period".into(), period);

        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?;
            params.insert("from".into(), datetime.to_rfc3339());
        }

        // Calculate till time based on limit
        if let Some(l) = limit {
            let duration = self.parse_timeframe(timeframe)?;
            if let Some(s) = since {
                let till_ms = s + ((l as i64) * duration * 1000);
                let till_datetime = chrono::DateTime::from_timestamp_millis(till_ms)
                    .ok_or_else(|| CcxtError::BadRequest {
                        message: "Invalid timestamp".into(),
                    })?;
                params.insert("till".into(), till_datetime.to_rfc3339());
            }
        }

        let response: Vec<TimexCandle> = self
            .public_get("/public/candles", Some(params))
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                let timestamp = c.timestamp
                    .as_ref()
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.timestamp_millis())?;

                Some(OHLCV {
                    timestamp,
                    open: c.open?,
                    high: c.high?,
                    low: c.low?,
                    close: c.close?,
                    volume: c.volume?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: Vec<TimexBalance> = self
            .private_request("GET", "/trading/balances", params)
            .await?;

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
        let market_id = {
            let markets = self.markets.read().unwrap();
            markets.get(symbol).ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.to_string(),
            })?.id.clone()
        };

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("quantity".into(), amount.to_string());
        params.insert("side".into(), match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        }.into());

        let order_type_str = match order_type {
            OrderType::Limit => "LIMIT",
            OrderType::Market => "MARKET",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            }),
        };
        params.insert("orderTypes".into(), order_type_str.into());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());

            // Timex requires expireIn for limit orders
            params.insert("expireIn".into(), "31536000".into()); // 1 year
        } else {
            params.insert("price".into(), "0".into());
        }

        let response: TimexCreateOrderResponse = self
            .private_request("POST", "/trading/orders", params)
            .await?;

        let order_data = response.orders.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No order returned".into(),
        })?;

        Ok(self.parse_order(order_data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("id".into(), id.to_string());

        let response: TimexCancelOrderResponse = self
            .private_request("DELETE", "/trading/orders", params)
            .await?;

        let changed_orders = response.changed_orders.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "No order returned".into(),
        })?;

        Ok(self.parse_order(&changed_orders.new_order, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderHash".into(), id.to_string());

        let response: TimexOrderDetails = self
            .private_request("GET", "/history/orders/details", params)
            .await?;

        Ok(self.parse_order(&response.order, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("sort".into(), "createdAt,asc".into());

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.to_string(),
            })?;
            params.insert("symbol".into(), market.id.clone());
        }

        if let Some(l) = limit {
            params.insert("size".into(), l.to_string());
        }

        let response: TimexOrdersResponse = self
            .private_request("GET", "/trading/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .orders
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("sort".into(), "createdAt,asc".into());
        params.insert("side".into(), "BUY".into()); // Timex requires side parameter

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.to_string(),
            })?;
            params.insert("symbol".into(), market.id.clone());
        }

        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?;
            params.insert("from".into(), datetime.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("size".into(), l.to_string());
        }

        let response: TimexOrdersResponse = self
            .private_request("GET", "/history/orders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let orders: Vec<Order> = response
            .orders
            .iter()
            .map(|o| {
                let sym = markets_by_id
                    .get(&o.symbol)
                    .cloned()
                    .unwrap_or_else(|| o.symbol.clone());
                self.parse_order(o, &sym)
            })
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let mut params = HashMap::new();
        params.insert("sort".into(), "timestamp,asc".into());

        if let Some(s) = symbol {
            let markets = self.markets.read().unwrap();
            let market = markets.get(s).ok_or_else(|| CcxtError::BadSymbol {
                symbol: s.to_string(),
            })?;
            params.insert("symbol".into(), market.id.clone());
        }

        if let Some(s) = since {
            let datetime = chrono::DateTime::from_timestamp_millis(s)
                .ok_or_else(|| CcxtError::BadRequest {
                    message: "Invalid timestamp".into(),
                })?;
            params.insert("from".into(), datetime.to_rfc3339());
        }

        if let Some(l) = limit {
            params.insert("size".into(), l.to_string());
        }

        let response: TimexTradesResponse = self
            .private_request("GET", "/history/trades", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap().clone();

        let trades: Vec<Trade> = response
            .trades
            .iter()
            .map(|t| {
                let sym = symbol.unwrap_or_else(|| {
                    markets_by_id
                        .get(&t.symbol.clone().unwrap_or_default())
                        .map(|s| s.as_str())
                        .unwrap_or("")
                });
                self.parse_trade(t, sym)
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
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" || api == "trading" || api == "history" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let auth_string = format!("{api_key}:{secret}");
            let encoded = BASE64.encode(auth_string);
            headers.insert("Authorization".into(), format!("Basic {encoded}"));
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

    async fn fetch_time(&self) -> CcxtResult<i64> {
        let response: i64 = self
            .public_client
            .get("/tradingview/time", None, None)
            .await?;

        Ok(response * 1000) // Convert to milliseconds
    }
}

// === Timex API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimexMarket {
    symbol: String,
    #[serde(default)]
    name: Option<String>,
    base_currency: String,
    quote_currency: String,
    #[serde(default)]
    quantity_increment: Option<Decimal>,
    #[serde(default)]
    taker_fee: Option<Decimal>,
    #[serde(default)]
    maker_fee: Option<Decimal>,
    #[serde(default)]
    tick_size: Option<Decimal>,
    #[serde(default)]
    base_min_size: Option<Decimal>,
    #[serde(default)]
    quote_min_size: Option<Decimal>,
    #[serde(default)]
    locked: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TimexTicker {
    #[serde(default)]
    market: Option<String>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    volume_quote: Option<Decimal>,
    #[serde(default, rename = "volumeQuote")]
    _volume_quote_alt: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimexOrderBook {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    bid: Vec<TimexOrderBookLevel>,
    #[serde(default)]
    ask: Vec<TimexOrderBookLevel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimexOrderBookLevel {
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    base_token_amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TimexTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    fee: Option<Decimal>,
    #[serde(default)]
    maker_or_taker: Option<String>,
    #[serde(default)]
    maker_order_id: Option<String>,
    #[serde(default)]
    taker_order_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimexCandle {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    open: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    close: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimexOrder {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    side: String,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    quantity: Option<Decimal>,
    #[serde(default)]
    filled_quantity: Option<Decimal>,
    #[serde(default)]
    cancelled_quantity: Option<Decimal>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    expire_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimexBalance {
    currency: String,
    #[serde(default, rename = "totalBalance")]
    total_balance: Option<Decimal>,
    #[serde(default, rename = "lockedBalance")]
    locked_balance: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct TimexCreateOrderResponse {
    orders: Vec<TimexOrder>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimexCancelOrderResponse {
    #[serde(default)]
    changed_orders: Vec<TimexChangedOrder>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimexChangedOrder {
    new_order: TimexOrder,
    #[serde(default)]
    old_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimexOrderDetails {
    order: TimexOrder,
    #[serde(default)]
    trades: Vec<TimexTrade>,
}

#[derive(Debug, Deserialize)]
struct TimexOrdersResponse {
    orders: Vec<TimexOrder>,
}

#[derive(Debug, Deserialize)]
struct TimexTradesResponse {
    trades: Vec<TimexTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TimexTransaction {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    transfer_hash: Option<String>,
    #[serde(default)]
    value: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let timex = Timex::new(config).unwrap();

        assert_eq!(timex.id(), ExchangeId::Timex);
        assert_eq!(timex.name(), "TimeX");
        assert!(timex.has().spot);
        assert!(!timex.has().margin);
        assert!(!timex.has().swap);
    }
}
