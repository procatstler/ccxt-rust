//! Binance US Exchange Implementation
//!
//! CCXT binanceus.py를 Rust로 포팅
//! Binance US는 미국 사용자를 위한 거래소로 현물 거래만 지원

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, TakerOrMaker, Ticker, TimeInForce, Timeframe, Trade, Transaction,
    OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// Binance US 거래소
pub struct BinanceUs {
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

impl BinanceUs {
    const BASE_URL: &'static str = "https://api.binance.us";
    const RATE_LIMIT_MS: u64 = 50; // 1200 requests per minute

    /// 새 BinanceUs 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        // Binance US는 현물만 지원 (마진, 선물 미지원)
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: true,
            fetch_deposit_address: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("{}/api/v3", Self::BASE_URL));
        api_urls.insert("private".into(), format!("{}/api/v3", Self::BASE_URL));
        api_urls.insert("sapi".into(), format!("{}/sapi/v1", Self::BASE_URL));

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/a9667919-b632-4d52-a832-df89f8a35e8c"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://www.binance.us".into()),
            doc: vec!["https://github.com/binance-us/binance-official-api-docs".into()],
            fees: Some("https://www.binance.us/en/fee/schedule".into()),
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

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: HashMap<String, String>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

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

        let timestamp = Utc::now().timestamp_millis().to_string();

        let mut query_params = params.clone();
        query_params.insert("timestamp".into(), timestamp);

        let query: String = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let signed_query = format!("{query}&signature={signature}");

        let mut headers = HashMap::new();
        headers.insert("X-MBX-APIKEY".into(), api_key.to_string());

        let url = format!("{path}?{signed_query}");

        match method {
            "GET" => self.private_client.get(&url, None, Some(headers)).await,
            "POST" => {
                headers.insert(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                );
                self.private_client
                    .post(&format!("{path}?{signed_query}"), None, Some(headers))
                    .await
            },
            "DELETE" => self.private_client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USD → BTCUSD)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BinanceUsTicker, symbol: &str) -> Ticker {
        let timestamp = data
            .close_time
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price,
            low: data.low_price,
            bid: data.bid_price,
            bid_volume: data.bid_qty,
            ask: data.ask_price,
            ask_volume: data.ask_qty,
            vwap: data.weighted_avg_price,
            open: data.open_price,
            close: data.last_price,
            last: data.last_price,
            previous_close: data.prev_close_price,
            change: data.price_change,
            percentage: data.price_change_percent,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BinanceUsOrder, symbol: &str) -> Order {
        let status = match data.status.as_str() {
            "NEW" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::Open,
            "FILLED" => OrderStatus::Closed,
            "CANCELED" => OrderStatus::Canceled,
            "PENDING_CANCEL" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            "EXPIRED" | "EXPIRED_IN_MATCH" => OrderStatus::Expired,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "LIMIT" => OrderType::Limit,
            "MARKET" => OrderType::Market,
            "LIMIT_MAKER" => OrderType::LimitMaker,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "BUY" => OrderSide::Buy,
            "SELL" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data
            .time_in_force
            .as_ref()
            .and_then(|tif| match tif.as_str() {
                "GTC" => Some(TimeInForce::GTC),
                "IOC" => Some(TimeInForce::IOC),
                "FOK" => Some(TimeInForce::FOK),
                _ => None,
            });

        let price: Option<Decimal> = data.price.as_ref().and_then(|p| p.parse().ok());
        let amount: Decimal = data.orig_qty.parse().unwrap_or_default();
        let filled: Decimal = data.executed_qty.parse().unwrap_or_default();
        let remaining = Some(amount - filled);
        let cost = data
            .cummulative_quote_qty
            .as_ref()
            .and_then(|c| c.parse().ok());
        let average = if filled > Decimal::ZERO {
            cost.map(|c| c / filled)
        } else {
            None
        };

        Order {
            id: data.order_id.to_string(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(data.time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: data.update_time,
            last_update_timestamp: data.update_time,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force,
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
    fn parse_balance(&self, balances: &[BinanceUsBalance]) -> Balances {
        let mut result = Balances::new();

        for b in balances {
            let free: Option<Decimal> = b.free.parse().ok();
            let used: Option<Decimal> = b.locked.parse().ok();
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
            result.add(&b.asset, balance);
        }

        result
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &BinanceUsDeposit) -> Transaction {
        let status = match data.status {
            0 => crate::types::TransactionStatus::Pending,
            1 => crate::types::TransactionStatus::Ok,
            6 => crate::types::TransactionStatus::Pending,
            _ => crate::types::TransactionStatus::Pending,
        };

        Transaction {
            id: data.id.clone().unwrap_or_default(),
            timestamp: Some(data.insert_time),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(data.insert_time)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            updated: None,
            tx_type: crate::types::TransactionType::Deposit,
            currency: data.coin.clone(),
            network: Some(data.network.clone()),
            amount: data.amount.parse().unwrap_or_default(),
            status,
            address: Some(data.address.clone()),
            tag: data.address_tag.clone(),
            txid: Some(data.tx_id.clone()),
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &BinanceUsWithdrawal) -> Transaction {
        let status = match data.status {
            0 => crate::types::TransactionStatus::Pending,
            1 => crate::types::TransactionStatus::Canceled,
            2 => crate::types::TransactionStatus::Pending,
            3 => crate::types::TransactionStatus::Failed,
            4 => crate::types::TransactionStatus::Pending,
            5 => crate::types::TransactionStatus::Failed,
            6 => crate::types::TransactionStatus::Ok,
            _ => crate::types::TransactionStatus::Pending,
        };

        let fee = data
            .transaction_fee
            .parse::<Decimal>()
            .ok()
            .map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: Some(data.coin.clone()),
                rate: None,
            });

        Transaction {
            id: data.id.clone(),
            timestamp: data.apply_time.parse::<i64>().ok(),
            datetime: Some(data.apply_time.clone()),
            updated: None,
            tx_type: crate::types::TransactionType::Withdrawal,
            currency: data.coin.clone(),
            network: Some(data.network.clone()),
            amount: data.amount.parse().unwrap_or_default(),
            status,
            address: Some(data.address.clone()),
            tag: data.address_tag.clone(),
            txid: data.tx_id.clone(),
            fee,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for BinanceUs {
    fn id(&self) -> ExchangeId {
        ExchangeId::BinanceUs
    }

    fn name(&self) -> &str {
        "Binance US"
    }

    fn version(&self) -> &str {
        "v3"
    }

    fn countries(&self) -> &[&str] {
        &["US"]
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
        let response: BinanceUsExchangeInfo = self.public_get("/api/v3/exchangeInfo", None).await?;

        let mut markets = Vec::new();

        for symbol_info in response.symbols {
            if symbol_info.status != "TRADING" {
                continue;
            }

            let base = symbol_info.base_asset.clone();
            let quote = symbol_info.quote_asset.clone();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: symbol_info.symbol.clone(),
                lowercase_id: Some(symbol_info.symbol.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: symbol_info.base_asset.clone(),
                quote_id: symbol_info.quote_asset.clone(),
                settle: None,
                settle_id: None,
                active: true,
                market_type: MarketType::Spot,
                spot: true,
                margin: false, // Binance US doesn't support margin
                swap: false,
                future: false,
                option: false,
                index: false,
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: Some(Decimal::new(1, 3)), // 0.1%
                maker: Some(Decimal::new(1, 3)), // 0.1%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(symbol_info.base_asset_precision),
                    price: Some(symbol_info.quote_precision),
                    cost: None,
                    base: Some(symbol_info.base_asset_precision),
                    quote: Some(symbol_info.quote_precision),
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&symbol_info).unwrap_or_default(),
                tier_based: true,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: BinanceUsTicker =
            self.public_get("/api/v3/ticker/24hr", Some(params)).await?;

        Ok(self.parse_ticker(&response, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: Vec<BinanceUsTicker> = self.public_get("/api/v3/ticker/24hr", None).await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let mut tickers = HashMap::new();

        for data in response {
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
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: BinanceUsOrderBook = self.public_get("/api/v3/depth", Some(params)).await?;

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
            timestamp: None,
            datetime: None,
            nonce: Some(response.last_update_id),
            bids,
            asks,
            checksum: None,
        })
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
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceUsTrade> = self.public_get("/api/v3/trades", Some(params)).await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| Trade {
                id: t.id.to_string(),
                order: None,
                timestamp: Some(t.time),
                datetime: Some(
                    chrono::DateTime::from_timestamp_millis(t.time)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                ),
                symbol: symbol.to_string(),
                trade_type: None,
                side: if t.is_buyer_maker {
                    Some("sell".into())
                } else {
                    Some("buy".into())
                },
                taker_or_maker: if t.is_buyer_maker {
                    Some(TakerOrMaker::Maker)
                } else {
                    Some(TakerOrMaker::Taker)
                },
                price: t.price.parse().unwrap_or_default(),
                amount: t.qty.parse().unwrap_or_default(),
                cost: Some(
                    t.price.parse::<Decimal>().unwrap_or_default()
                        * t.qty.parse::<Decimal>().unwrap_or_default(),
                ),
                fee: None,
                fees: Vec::new(),
                info: serde_json::to_value(t).unwrap_or_default(),
            })
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
        let market_id = self.to_market_id(symbol);
        let interval = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
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

        let response: Vec<Vec<serde_json::Value>> =
            self.public_get("/api/v3/klines", Some(params)).await?;

        let ohlcv: Vec<OHLCV> = response
            .iter()
            .filter_map(|c| {
                if c.len() < 6 {
                    return None;
                }
                Some(OHLCV {
                    timestamp: c[0].as_i64()?,
                    open: c[1].as_str()?.parse().ok()?,
                    high: c[2].as_str()?.parse().ok()?,
                    low: c[3].as_str()?.parse().ok()?,
                    close: c[4].as_str()?.parse().ok()?,
                    volume: c[5].as_str()?.parse().ok()?,
                })
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let params = HashMap::new();

        let response: BinanceUsAccountInfo = self
            .private_request("GET", "/api/v3/account", params)
            .await?;

        Ok(self.parse_balance(&response.balances))
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
        params.insert(
            "side".into(),
            match side {
                OrderSide::Buy => "BUY",
                OrderSide::Sell => "SELL",
            }
            .into(),
        );
        params.insert(
            "type".into(),
            match order_type {
                OrderType::Limit => "LIMIT",
                OrderType::Market => "MARKET",
                OrderType::LimitMaker => "LIMIT_MAKER",
                _ => {
                    return Err(CcxtError::NotSupported {
                        feature: format!("Order type: {order_type:?}"),
                    })
                },
            }
            .into(),
        );
        params.insert("quantity".into(), amount.to_string());

        if order_type == OrderType::Limit || order_type == OrderType::LimitMaker {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("price".into(), price_val.to_string());
            params.insert("timeInForce".into(), "GTC".into());
        }

        let response: BinanceUsOrder = self
            .private_request("POST", "/api/v3/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BinanceUsOrder = self
            .private_request("DELETE", "/api/v3/order", params)
            .await?;

        Ok(self.parse_order(&response, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);
        params.insert("orderId".into(), id.to_string());

        let response: BinanceUsOrder = self.private_request("GET", "/api/v3/order", params).await?;

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
            params.insert("symbol".into(), self.to_market_id(s));
        }

        let response: Vec<BinanceUsOrder> = self
            .private_request("GET", "/api/v3/openOrders", params)
            .await?;

        let markets_by_id = self.markets_by_id.read().unwrap();

        let orders: Vec<Order> = response
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

    async fn cancel_all_orders(&self, symbol: Option<&str>) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Binance US cancelAllOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        let response: Vec<BinanceUsOrder> = self
            .private_request("DELETE", "/api/v3/openOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Binance US fetchMyTrades requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceUsMyTrade> = self
            .private_request("GET", "/api/v3/myTrades", params)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .map(|t| {
                let price: Decimal = t.price.parse().unwrap_or_default();
                let amount: Decimal = t.qty.parse().unwrap_or_default();
                let cost = price * amount;

                Trade {
                    id: t.id.to_string(),
                    order: Some(t.order_id.to_string()),
                    timestamp: Some(t.time),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(t.time)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: if t.is_buyer {
                        Some("buy".into())
                    } else {
                        Some("sell".into())
                    },
                    taker_or_maker: if t.is_maker {
                        Some(TakerOrMaker::Maker)
                    } else {
                        Some(TakerOrMaker::Taker)
                    },
                    price,
                    amount,
                    cost: Some(cost),
                    fee: Some(crate::types::Fee {
                        cost: t.commission.parse().ok(),
                        currency: Some(t.commission_asset.clone()),
                        rate: None,
                    }),
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Binance US fetchClosedOrders requires a symbol".into(),
        })?;

        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("symbol".into(), market_id);

        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceUsOrder> = self
            .private_request("GET", "/api/v3/allOrders", params)
            .await?;

        let orders: Vec<Order> = response
            .iter()
            .filter(|o| {
                o.status == "FILLED"
                    || o.status == "CANCELED"
                    || o.status == "EXPIRED"
                    || o.status == "REJECTED"
            })
            .map(|o| self.parse_order(o, symbol))
            .collect();

        Ok(orders)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("coin".into(), c.to_string());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceUsDeposit> = self
            .private_request("GET", "/sapi/v1/capital/deposit/hisrec", params)
            .await?;

        let transactions: Vec<Transaction> =
            response.iter().map(|d| self.parse_deposit(d)).collect();

        Ok(transactions)
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();

        if let Some(c) = code {
            params.insert("coin".into(), c.to_string());
        }
        if let Some(s) = since {
            params.insert("startTime".into(), s.to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(1000).to_string());
        }

        let response: Vec<BinanceUsWithdrawal> = self
            .private_request("GET", "/sapi/v1/capital/withdraw/history", params)
            .await?;

        let transactions: Vec<Transaction> =
            response.iter().map(|w| self.parse_withdrawal(w)).collect();

        Ok(transactions)
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

        let response: BinanceUsDepositAddress = self
            .private_request("GET", "/sapi/v1/capital/deposit/address", params)
            .await?;

        Ok(
            crate::types::DepositAddress::new(response.coin, response.address)
                .with_network(response.network)
                .with_tag(response.tag),
        )
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
        params.insert("coin".into(), code.to_string());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("addressTag".into(), t.to_string());
        }
        if let Some(n) = network {
            params.insert("network".into(), n.to_string());
        }

        let response: BinanceUsWithdrawResponse = self
            .private_request("POST", "/sapi/v1/capital/withdraw/apply", params)
            .await?;

        Ok(Transaction {
            id: response.id,
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            updated: None,
            tx_type: crate::types::TransactionType::Withdrawal,
            currency: code.to_string(),
            network: network.map(String::from),
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
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}{}", Self::BASE_URL, path);
        let mut headers = HashMap::new();

        if api == "private" {
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let timestamp = Utc::now().timestamp_millis().to_string();
            let mut query_params = params.clone();
            query_params.insert("timestamp".into(), timestamp);

            let query: String = query_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(query.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());

            url = format!("{url}?{query}&signature={signature}");
            headers.insert("X-MBX-APIKEY".into(), api_key.to_string());
        } else if !params.is_empty() {
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
}

// === Binance US API Response Types ===

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsExchangeInfo {
    symbols: Vec<BinanceUsSymbol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsSymbol {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
    base_asset_precision: i32,
    quote_precision: i32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsTicker {
    symbol: String,
    #[serde(default)]
    price_change: Option<Decimal>,
    #[serde(default)]
    price_change_percent: Option<Decimal>,
    #[serde(default)]
    weighted_avg_price: Option<Decimal>,
    #[serde(default)]
    prev_close_price: Option<Decimal>,
    #[serde(default)]
    last_price: Option<Decimal>,
    #[serde(default)]
    bid_price: Option<Decimal>,
    #[serde(default)]
    bid_qty: Option<Decimal>,
    #[serde(default)]
    ask_price: Option<Decimal>,
    #[serde(default)]
    ask_qty: Option<Decimal>,
    #[serde(default)]
    open_price: Option<Decimal>,
    #[serde(default)]
    high_price: Option<Decimal>,
    #[serde(default)]
    low_price: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    quote_volume: Option<Decimal>,
    #[serde(default)]
    open_time: Option<i64>,
    #[serde(default)]
    close_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsOrderBook {
    last_update_id: i64,
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsTrade {
    id: i64,
    price: String,
    qty: String,
    time: i64,
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsOrder {
    symbol: String,
    order_id: i64,
    #[serde(default)]
    client_order_id: Option<String>,
    price: Option<String>,
    orig_qty: String,
    executed_qty: String,
    #[serde(default)]
    cummulative_quote_qty: Option<String>,
    status: String,
    #[serde(rename = "type")]
    order_type: String,
    side: String,
    #[serde(default)]
    time_in_force: Option<String>,
    time: i64,
    #[serde(default)]
    update_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsAccountInfo {
    balances: Vec<BinanceUsBalance>,
}

#[derive(Debug, Deserialize)]
struct BinanceUsBalance {
    asset: String,
    free: String,
    locked: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsMyTrade {
    id: i64,
    order_id: i64,
    price: String,
    qty: String,
    commission: String,
    commission_asset: String,
    time: i64,
    is_buyer: bool,
    is_maker: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsDeposit {
    #[serde(default)]
    id: Option<String>,
    coin: String,
    amount: String,
    network: String,
    address: String,
    #[serde(default)]
    address_tag: Option<String>,
    tx_id: String,
    insert_time: i64,
    status: i32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsWithdrawal {
    id: String,
    coin: String,
    amount: String,
    network: String,
    address: String,
    #[serde(default)]
    address_tag: Option<String>,
    #[serde(default)]
    tx_id: Option<String>,
    apply_time: String,
    transaction_fee: String,
    status: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsDepositAddress {
    coin: String,
    address: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    network: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceUsWithdrawResponse {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let binance_us = BinanceUs::new(config).unwrap();

        assert_eq!(binance_us.to_market_id("BTC/USD"), "BTCUSD");
        assert_eq!(binance_us.to_market_id("ETH/USDT"), "ETHUSDT");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let binance_us = BinanceUs::new(config).unwrap();

        assert_eq!(binance_us.id(), ExchangeId::BinanceUs);
        assert_eq!(binance_us.name(), "Binance US");
        assert!(binance_us.has().spot);
        assert!(!binance_us.has().margin); // Binance US doesn't support margin
        assert!(!binance_us.has().swap); // No derivatives
    }
}
