//! Coinone Exchange Implementation
//!
//! CCXT coinone.ts를 Rust로 포팅

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::RwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, MarketLimits,
    MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide, OrderStatus,
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha512 = Hmac<Sha512>;

/// Coinone 거래소
pub struct Coinone {
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

impl Coinone {
    const PUBLIC_URL: &'static str = "https://api.coinone.co.kr/public/v2";
    const PRIVATE_URL: &'static str = "https://api.coinone.co.kr/v2";
    const RATE_LIMIT_MS: u64 = 50; // 초당 20회

    /// 새 Coinone 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::PUBLIC_URL, &config)?;
        let private_client = HttpClient::new(Self::PRIVATE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            fetch_markets: true,
            fetch_currencies: true,
            fetch_ticker: true,
            fetch_tickers: true,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            fetch_deposit_address: true,
            ws: true,
            ..Default::default()
        };

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/38003300-adc12fba-323f-11e8-8525-725f53c4a659.jpg".into()),
            api: HashMap::from([
                ("public".into(), Self::PUBLIC_URL.into()),
                ("private".into(), Self::PRIVATE_URL.into()),
            ]),
            www: Some("https://coinone.co.kr".into()),
            doc: vec!["https://doc.coinone.co.kr".into()],
            fees: Some("https://coinone.co.kr/help/fees".into()),
        };

        // Coinone doesn't support OHLCV in the main API
        let timeframes = HashMap::new();

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

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        mut params: serde_json::Value,
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

        let nonce = Utc::now().timestamp_millis().to_string();

        // Add access_token and nonce to params
        if let serde_json::Value::Object(ref mut map) = params {
            map.insert(
                "access_token".into(),
                serde_json::Value::String(api_key.to_string()),
            );
            map.insert("nonce".into(), serde_json::Value::String(nonce));
        }

        // Create payload (base64 encoded JSON)
        let json_str = serde_json::to_string(&params).unwrap_or_default();
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            json_str.as_bytes(),
        );

        // Create signature: HMAC-SHA512 of payload using uppercase secret
        let secret_upper = secret.to_uppercase();
        let mut mac = HmacSha512::new_from_slice(secret_upper.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("X-COINONE-PAYLOAD".into(), payload.clone());
        headers.insert("X-COINONE-SIGNATURE".into(), signature);

        let url = path.to_string();
        self.private_client
            .post(&url, Some(serde_json::json!(&payload)), Some(headers))
            .await
    }

    /// 잔고 응답 파싱
    fn parse_balance_response(&self, data: &serde_json::Value) -> Balances {
        let mut balances = Balances::new();

        if let Some(obj) = data.as_object() {
            for (key, value) in obj {
                // Skip metadata fields
                if key == "result" || key == "errorCode" || key == "normalWallets" {
                    continue;
                }

                if let Some(currency_data) = value.as_object() {
                    let total: Option<Decimal> = currency_data
                        .get("balance")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok());
                    let free: Option<Decimal> = currency_data
                        .get("avail")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok());
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
                    balances.add(key.to_uppercase(), balance);
                }
            }
        }

        balances
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "live" | "partially_filled" => OrderStatus::Open,
            "filled" => OrderStatus::Closed,
            "canceled" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// 주문 응답 파싱
    fn parse_order_response(&self, data: &serde_json::Value, symbol: &str) -> Order {
        let id = data
            .get("orderId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| self.parse_order_status(s))
            .unwrap_or(OrderStatus::Open);
        let side = match data.get("side").and_then(|v| v.as_str()) {
            Some("bid") => OrderSide::Buy,
            Some("ask") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let price: Option<Decimal> = data
            .get("price")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        let amount: Decimal = data
            .get("originalQty")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let filled: Decimal = data
            .get("executedQty")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        let remaining: Option<Decimal> = data
            .get("remainQty")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());

        let timestamp = data
            .get("orderedAt")
            .and_then(|v| v.as_i64())
            .map(|t| t * 1000);

        Order {
            id,
            client_order_id: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price,
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
            post_only: None,
            info: data.clone(),
        }
    }

    /// 심볼을 market id 형식으로 변환 (BTC/KRW → (KRW, BTC))
    fn to_market_parts(&self, symbol: &str) -> (String, String) {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            // quote_currency, target_currency
            (parts[1].to_uppercase(), parts[0].to_uppercase())
        } else {
            ("KRW".to_string(), symbol.to_uppercase())
        }
    }

    /// market id를 심볼로 변환 ((KRW, BTC) → BTC/KRW)
    fn to_symbol_from_parts(&self, quote: &str, target: &str) -> String {
        format!("{}/{}", target.to_uppercase(), quote.to_uppercase())
    }

    /// 티커 데이터 파싱
    fn parse_ticker(&self, data: &serde_json::Value, symbol: &str) -> CcxtResult<Ticker> {
        let timestamp = data.get("timestamp").and_then(|v| v.as_i64());

        let high = data
            .get("high")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok());
        let low = data
            .get("low")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok());
        let last = data
            .get("last")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok());
        let open = data
            .get("first")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok());
        let base_volume = data
            .get("target_volume")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok());
        let quote_volume = data
            .get("quote_volume")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok());

        // Best bid/ask
        let best_bids = data.get("best_bids").and_then(|v| v.as_array());
        let best_asks = data.get("best_asks").and_then(|v| v.as_array());

        let (bid, bid_volume) = best_bids
            .and_then(|arr| arr.first())
            .map(|b| {
                let price = b
                    .get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok());
                let qty = b
                    .get("qty")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok());
                (price, qty)
            })
            .unwrap_or((None, None));

        let (ask, ask_volume) = best_asks
            .and_then(|arr| arr.first())
            .map(|a| {
                let price = a
                    .get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok());
                let qty = a
                    .get("qty")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok());
                (price, qty)
            })
            .unwrap_or((None, None));

        let datetime = timestamp
            .and_then(|ts| chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339()));

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp,
            datetime,
            high,
            low,
            bid,
            bid_volume,
            ask,
            ask_volume,
            vwap: None,
            open,
            close: last,
            last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume,
            quote_volume,
            index_price: None,
            mark_price: None,
            info: data.clone(),
        })
    }

    /// 거래 데이터 파싱
    fn parse_trade(&self, data: &serde_json::Value, symbol: &str) -> CcxtResult<Trade> {
        let id = data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let timestamp = data.get("timestamp").and_then(|v| v.as_i64());

        let price = data
            .get("price")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or(Decimal::ZERO);

        let amount = data
            .get("qty")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Decimal>().ok())
            .unwrap_or(Decimal::ZERO);

        let is_seller_maker = data
            .get("is_seller_maker")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let side = if is_seller_maker { "sell" } else { "buy" };

        let datetime = timestamp
            .and_then(|ts| chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339()));

        Ok(Trade {
            id,
            order: None,
            timestamp,
            datetime,
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: data.clone(),
        })
    }
}

#[async_trait]
impl Exchange for Coinone {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coinone
    }

    fn name(&self) -> &str {
        "Coinone"
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

        self.rate_limiter.throttle(1.0).await;

        // Coinone uses ticker endpoint to get markets
        let url = format!("{}/ticker_new/KRW", Self::PUBLIC_URL);
        let response: CoinoneTickersResponse = self.public_client.get(&url, None, None).await?;

        if response.result != "success" {
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to load markets: {}", response.error_code),
            });
        }

        let mut markets = HashMap::new();
        let mut markets_by_id = HashMap::new();

        for ticker in response.tickers {
            let base = ticker.target_currency.to_uppercase();
            let quote = ticker.quote_currency.to_uppercase();
            let symbol = format!("{base}/{quote}");
            let id = ticker.id.clone();

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.to_lowercase()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: ticker.target_currency.clone(),
                quote_id: ticker.quote_currency.clone(),
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
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                taker: Some(Decimal::new(2, 3)), // 0.2%
                maker: Some(Decimal::new(2, 3)), // 0.2%
                percentage: true,
                tier_based: false,
                precision: MarketPrecision {
                    amount: Some(4),
                    price: Some(4),
                    cost: Some(8),
                    base: Some(8),
                    quote: Some(0),
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&ticker).unwrap_or_default(),
            };

            markets_by_id.insert(id, symbol.clone());
            markets.insert(symbol, market);
        }

        // Update cache
        {
            let mut cache = self.markets.write().unwrap();
            *cache = markets.clone();
        }
        {
            let mut cache = self.markets_by_id.write().unwrap();
            *cache = markets_by_id;
        }

        Ok(markets)
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        let markets = self.load_markets(false).await?;
        Ok(markets.into_values().collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.rate_limiter.throttle(1.0).await;

        let (quote, target) = self.to_market_parts(symbol);
        let url = format!("{}/ticker_new/{}/{}", Self::PUBLIC_URL, quote, target);

        let response: serde_json::Value = self.public_client.get(&url, None, None).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to fetch ticker: {:?}", response.get("error_code")),
            });
        }

        let tickers = response
            .get("tickers")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Invalid ticker response".into(),
            })?;

        let ticker_data = tickers.first().ok_or_else(|| CcxtError::ExchangeError {
            message: "Empty ticker response".into(),
        })?;

        self.parse_ticker(ticker_data, symbol)
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.rate_limiter.throttle(1.0).await;

        let url = format!("{}/ticker_new/KRW", Self::PUBLIC_URL);
        let response: serde_json::Value = self.public_client.get(&url, None, None).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to fetch tickers: {:?}", response.get("error_code")),
            });
        }

        let tickers_data = response
            .get("tickers")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Invalid tickers response".into(),
            })?;

        let mut tickers = HashMap::new();

        for data in tickers_data {
            let target = data
                .get("target_currency")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let quote = data
                .get("quote_currency")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let symbol = self.to_symbol_from_parts(quote, target);

            // Filter if symbols specified
            if let Some(filter) = symbols {
                if !filter.iter().any(|s| *s == symbol) {
                    continue;
                }
            }

            if let Ok(ticker) = self.parse_ticker(data, &symbol) {
                tickers.insert(symbol, ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.rate_limiter.throttle(1.0).await;

        let (quote, target) = self.to_market_parts(symbol);
        let mut url = format!("{}/orderbook/{}/{}", Self::PUBLIC_URL, quote, target);

        if let Some(l) = limit {
            url = format!("{url}?size={l}");
        }

        let response: CoinoneOrderBookResponse = self.public_client.get(&url, None, None).await?;

        if response.result != "success" {
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to fetch order book: {}", response.error_code),
            });
        }

        let bids: Vec<OrderBookEntry> = response
            .bids
            .iter()
            .map(|bid| OrderBookEntry {
                price: bid.price.parse().unwrap_or(Decimal::ZERO),
                amount: bid.qty.parse().unwrap_or(Decimal::ZERO),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .asks
            .iter()
            .map(|ask| OrderBookEntry {
                price: ask.price.parse().unwrap_or(Decimal::ZERO),
                amount: ask.qty.parse().unwrap_or(Decimal::ZERO),
            })
            .collect();

        let datetime =
            chrono::DateTime::from_timestamp_millis(response.timestamp).map(|dt| dt.to_rfc3339());

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            checksum: None,
            timestamp: Some(response.timestamp),
            datetime,
            nonce: None,
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.rate_limiter.throttle(1.0).await;

        let (quote, target) = self.to_market_parts(symbol);
        let mut url = format!("{}/trades/{}/{}", Self::PUBLIC_URL, quote, target);

        if let Some(l) = limit {
            let size = std::cmp::min(l, 200);
            url = format!("{url}?size={size}");
        }

        let response: serde_json::Value = self.public_client.get(&url, None, None).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Failed to fetch trades: {:?}", response.get("error_code")),
            });
        }

        let transactions = response
            .get("transactions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CcxtError::ExchangeError {
                message: "Invalid trades response".into(),
            })?;

        let mut trades = Vec::new();

        for data in transactions {
            if let Ok(trade) = self.parse_trade(data, symbol) {
                // Filter by since if specified
                if let Some(s) = since {
                    if let Some(ts) = trade.timestamp {
                        if ts < s {
                            continue;
                        }
                    }
                }
                trades.push(trade);
            }
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
        // Coinone doesn't support OHLCV in public API
        Err(CcxtError::NotSupported {
            feature: "fetchOHLCV".into(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: serde_json::Value = self
            .private_request("/account/balance", serde_json::json!({}))
            .await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Error fetching balance: {:?}", response.get("errorCode")),
            });
        }

        Ok(self.parse_balance_response(&response))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        // Coinone only supports limit orders
        if order_type != OrderType::Limit {
            return Err(CcxtError::NotSupported {
                feature: format!("Order type: {order_type:?}"),
            });
        }

        let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Coinone requires price for limit orders".into(),
        })?;

        let (_quote, target) = self.to_market_parts(symbol);

        let params = serde_json::json!({
            "price": price_val.to_string(),
            "qty": amount.to_string(),
            "currency": target.to_lowercase(),
        });

        let endpoint = match side {
            OrderSide::Buy => "/order/limit_buy",
            OrderSide::Sell => "/order/limit_sell",
        };

        let response: serde_json::Value = self.private_request(endpoint, params).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Error creating order: {:?}", response.get("errorCode")),
            });
        }

        let order_id = response
            .get("orderId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

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
            price: Some(price_val),
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
            info: response,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // Coinone cancel requires price, qty, and is_ask
        // We need to fetch the order first to get these details
        let order = self.fetch_order(id, symbol).await?;

        let is_ask = match order.side {
            OrderSide::Sell => 1,
            OrderSide::Buy => 0,
        };

        let (_quote, target) = self.to_market_parts(symbol);

        let params = serde_json::json!({
            "order_id": id,
            "price": order.price.map(|p| p.to_string()).unwrap_or_default(),
            "qty": order.amount.to_string(),
            "is_ask": is_ask,
            "currency": target.to_lowercase(),
        });

        let response: serde_json::Value = self.private_request("/order/cancel", params).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Error canceling order: {:?}", response.get("errorCode")),
            });
        }

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: order.timestamp,
            datetime: order.datetime,
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Canceled,
            symbol: symbol.to_string(),
            order_type: order.order_type,
            time_in_force: None,
            side: order.side,
            price: order.price,
            average: None,
            amount: order.amount,
            filled: order.filled,
            remaining: order.remaining,
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
            info: response,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let (_quote, target) = self.to_market_parts(symbol);

        let params = serde_json::json!({
            "order_id": id,
            "currency": target.to_lowercase(),
        });

        let response: serde_json::Value =
            self.private_request("/order/query_order", params).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Error fetching order: {:?}", response.get("errorCode")),
            });
        }

        Ok(self.parse_order_response(&response, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let symbol_str = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Coinone requires a symbol for fetchOpenOrders".into(),
        })?;

        let (_quote, target) = self.to_market_parts(symbol_str);

        let params = serde_json::json!({
            "currency": target.to_lowercase(),
        });

        let response: serde_json::Value =
            self.private_request("/order/limit_orders", params).await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!(
                    "Error fetching open orders: {:?}",
                    response.get("errorCode")
                ),
            });
        }

        let limit_orders = response
            .get("limitOrders")
            .and_then(|v| v.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        Ok(limit_orders
            .iter()
            .map(|o| self.parse_order_response(o, symbol_str))
            .collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let symbol_str = symbol.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Coinone requires a symbol for fetchMyTrades".into(),
        })?;

        let (_quote, target) = self.to_market_parts(symbol_str);

        let params = serde_json::json!({
            "currency": target.to_lowercase(),
        });

        let response: serde_json::Value = self
            .private_request("/order/complete_orders", params)
            .await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!("Error fetching my trades: {:?}", response.get("errorCode")),
            });
        }

        let complete_orders = response
            .get("completeOrders")
            .and_then(|v| v.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default();

        let mut trades = Vec::new();

        for order_data in &complete_orders {
            let id = order_data
                .get("orderId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let timestamp = order_data
                .get("timestamp")
                .and_then(|v| v.as_i64())
                .map(|t| t * 1000);

            let price: Decimal = order_data
                .get("price")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default();

            let amount: Decimal = order_data
                .get("qty")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or_default();

            let fee_amount: Option<Decimal> = order_data
                .get("fee")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok());

            let side = match order_data.get("type").and_then(|v| v.as_str()) {
                Some("bid") => Some("buy".to_string()),
                Some("ask") => Some("sell".to_string()),
                _ => None,
            };

            let fee = fee_amount.map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: Some("KRW".to_string()),
                rate: None,
            });

            let trade = Trade {
                id: id.clone(),
                order: Some(id),
                timestamp,
                datetime: timestamp.and_then(|ts| {
                    chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
                }),
                symbol: symbol_str.to_string(),
                trade_type: None,
                side,
                taker_or_maker: None,
                price,
                amount,
                cost: Some(price * amount),
                fee,
                fees: Vec::new(),
                info: order_data.clone(),
            };

            trades.push(trade);
        }

        Ok(trades)
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        _network: Option<&str>,
    ) -> CcxtResult<crate::types::DepositAddress> {
        let params = serde_json::json!({
            "currency": code.to_lowercase(),
        });

        let response: serde_json::Value = self
            .private_request("/account/deposit_address", params)
            .await?;

        if response.get("result").and_then(|v| v.as_str()) != Some("success") {
            return Err(CcxtError::ExchangeError {
                message: format!(
                    "Error fetching deposit address: {:?}",
                    response.get("errorCode")
                ),
            });
        }

        let address = response
            .get("walletAddress")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let tag = response
            .get("memoId")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut deposit_address = crate::types::DepositAddress::new(code, address);
        if let Some(t) = tag {
            deposit_address = deposit_address.with_tag(t);
        }

        Ok(deposit_address)
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
        let url = if api == "public" {
            format!("{}{}", Self::PUBLIC_URL, path)
        } else {
            format!("{}{}", Self::PRIVATE_URL, path)
        };

        let mut headers = HashMap::new();

        if api != "public" {
            let nonce = Utc::now().timestamp_millis().to_string();

            // Build JSON payload
            let mut payload_data: HashMap<String, serde_json::Value> = HashMap::new();
            if let Some(api_key) = self.config.api_key() {
                payload_data.insert("access_token".into(), serde_json::json!(api_key));
            }
            payload_data.insert("nonce".into(), serde_json::json!(nonce));

            for (k, v) in params {
                payload_data.insert(k.clone(), serde_json::json!(v));
            }

            let json_str = serde_json::to_string(&payload_data).unwrap_or_default();
            let payload = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                json_str.as_bytes(),
            );

            if let Some(secret) = self.config.secret() {
                let secret_upper = secret.to_uppercase();
                let mut mac = HmacSha512::new_from_slice(secret_upper.as_bytes())
                    .expect("HMAC can take key of any size");
                mac.update(payload.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());

                headers.insert("Content-Type".into(), "application/json".into());
                headers.insert("X-COINONE-PAYLOAD".into(), payload.clone());
                headers.insert("X-COINONE-SIGNATURE".into(), signature);
            }
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }
}

// Response types

#[derive(Debug, Deserialize)]
struct CoinoneTickersResponse {
    result: String,
    error_code: String,
    #[serde(default)]
    tickers: Vec<CoinoneTicker>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoinoneTicker {
    #[serde(default)]
    id: String,
    quote_currency: String,
    target_currency: String,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    high: String,
    #[serde(default)]
    low: String,
    #[serde(default)]
    first: String,
    #[serde(default)]
    last: String,
    #[serde(default)]
    quote_volume: String,
    #[serde(default)]
    target_volume: String,
}

#[derive(Debug, Deserialize)]
struct CoinoneOrderBookResponse {
    result: String,
    error_code: String,
    timestamp: i64,
    #[serde(default)]
    bids: Vec<CoinoneOrderBookEntry>,
    #[serde(default)]
    asks: Vec<CoinoneOrderBookEntry>,
}

#[derive(Debug, Deserialize)]
struct CoinoneOrderBookEntry {
    price: String,
    qty: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Coinone::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Coinone);
        assert_eq!(exchange.name(), "Coinone");
        assert!(exchange.has().spot);
        assert!(!exchange.has().swap);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Coinone::new(config).unwrap();

        let (quote, target) = exchange.to_market_parts("BTC/KRW");
        assert_eq!(quote, "KRW");
        assert_eq!(target, "BTC");

        let symbol = exchange.to_symbol_from_parts("krw", "btc");
        assert_eq!(symbol, "BTC/KRW");
    }
}
