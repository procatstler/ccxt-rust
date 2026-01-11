//! CEX.IO Exchange Implementation
//!
//! CCXT cex.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use base64::Engine;
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
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, TimeInForce, Trade, OHLCV,
};

type HmacSha256 = Hmac<Sha256>;

/// CEX.IO 거래소
pub struct Cex {
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

impl Cex {
    const BASE_URL_PUBLIC: &'static str = "https://trade.cex.io/api/spot/rest-public";
    const BASE_URL_PRIVATE: &'static str = "https://trade.cex.io/api/spot/rest";
    const RATE_LIMIT_MS: u64 = 300; // 200 req/min

    /// 새 CEX.IO 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::BASE_URL_PUBLIC, &config)?;
        let private_client = HttpClient::new(Self::BASE_URL_PRIVATE, &config)?;
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_my_trades: false,
            fetch_deposits: true,
            fetch_withdrawals: true,
            withdraw: false,
            fetch_deposit_address: true,
            fetch_ledger: true,
            transfer: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL_PUBLIC.into());
        api_urls.insert("private".into(), Self::BASE_URL_PRIVATE.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766442-8ddc33b0-5ed8-11e7-8b98-f786aef0f3c9.jpg".into()),
            api: api_urls,
            www: Some("https://cex.io".into()),
            doc: vec!["https://trade.cex.io/docs/".into()],
            fees: Some("https://cex.io/fee-schedule".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "15m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour2, "2h".into());
        timeframes.insert(Timeframe::Hour4, "4h".into());
        timeframes.insert(Timeframe::Day1, "1d".into());

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
    async fn public_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());

        self.public_client.post(path, Some(serde_json::to_value(params).unwrap_or(serde_json::json!({}))), Some(headers)).await
    }

    /// 비공개 API 호출 (HMAC-SHA256 서명)
    async fn private_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let seconds = Utc::now().timestamp().to_string();
        let body = serde_json::to_string(&params).map_err(|e| CcxtError::ExchangeError {
            message: format!("Failed to serialize params: {e}"),
        })?;

        // Create signature: HMAC-SHA256(path + seconds + body, secret)
        let auth_string = format!("{path}{seconds}{body}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(auth_string.as_bytes());
        let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("X-AGGR-KEY".into(), api_key.to_string());
        headers.insert("X-AGGR-TIMESTAMP".into(), seconds);
        headers.insert("X-AGGR-SIGNATURE".into(), signature);

        self.private_client.post(path, Some(serde_json::to_value(params).unwrap_or(serde_json::json!({}))), Some(headers)).await
    }

    /// 심볼 → 마켓 ID 변환 (BTC/USDT → BTC-USDT)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 마켓 ID → 심볼 변환 (BTC-USDT → BTC/USDT)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &CexTicker, symbol: &str) -> Ticker {
        Ticker {
            symbol: symbol.to_string(),
            timestamp: None,
            datetime: None,
            high: data.high,
            low: data.low,
            bid: data.best_bid,
            bid_volume: None,
            ask: data.best_ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last,
            last: data.last,
            previous_close: None,
            change: data.price_change,
            percentage: data.price_change_percentage,
            average: None,
            base_volume: data.volume,
            quote_volume: data.quote_volume,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &CexOrder, symbol: &str) -> Order {
        let status = match data.order_status.as_str() {
            "a" => OrderStatus::Open,
            "d" => OrderStatus::Closed,
            "c" | "cd" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let order_type = match data.order_type.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            "stopLimit" => OrderType::StopLossLimit,
            _ => OrderType::Limit,
        };

        let side = match data.side.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let time_in_force = data.time_in_force.as_ref().and_then(|tif| match tif.as_str() {
            "GTC" => Some(TimeInForce::GTC),
            "IOC" => Some(TimeInForce::IOC),
            "FOK" => Some(TimeInForce::FOK),
            _ => None,
        });

        let timestamp = data.timestamp_ms;
        let amount = data.remains.unwrap_or(Decimal::ZERO) + data.executed_amount.unwrap_or(Decimal::ZERO);
        let filled = data.executed_amount.unwrap_or(Decimal::ZERO);
        let remaining = Some(data.remains.unwrap_or(Decimal::ZERO));

        Order {
            id: data.order_id.clone(),
            client_order_id: data.client_order_id.clone(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol: symbol.to_string(),
            order_type,
            time_in_force,
            side,
            price: data.price,
            average: None,
            amount,
            filled,
            remaining,
            stop_price: data.stop_price,
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

    /// 잔고 응답 파싱
    fn parse_balance(&self, balances: &HashMap<String, CexBalance>) -> Balances {
        let mut result = Balances::new();

        for (currency, balance) in balances {
            let free = balance.available;
            let used = balance.orders;
            let total = free.and_then(|f| used.map(|u| f + u));

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
}

#[async_trait]
impl Exchange for Cex {
    fn id(&self) -> ExchangeId {
        ExchangeId::Cex
    }

    fn name(&self) -> &str {
        "CEX.IO"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["GB", "EU", "CY", "RU"]
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
        let response: CexPairsInfoResponse = self
            .public_post("/get_pairs_info", HashMap::new())
            .await?;

        let mut markets = Vec::new();
        let pairs = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No pairs data in response".into(),
        })?;

        for pair_info in pairs {
            let symbol = pair_info.symbol1.clone() + "/" + &pair_info.symbol2;
            let market_id = pair_info.symbol1.clone() + "-" + &pair_info.symbol2;

            let market = Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: pair_info.symbol1.clone(),
                quote: pair_info.symbol2.clone(),
                base_id: pair_info.symbol1.clone(),
                quote_id: pair_info.symbol2.clone(),
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
                taker: Some(Decimal::new(25, 4)), // 0.25%
                maker: Some(Decimal::new(25, 4)), // 0.25%
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                precision: MarketPrecision {
                    amount: Some(pair_info.min_lot_size_s2),
                    price: Some(pair_info.min_price_increment),
                    cost: None,
                    base: Some(pair_info.min_lot_size_s2),
                    quote: Some(pair_info.min_price_increment),
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&pair_info).unwrap_or_default(),
                tier_based: false,
                percentage: true,
            };

            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), serde_json::Value::String(market_id.clone()));

        let response: CexTickerResponse = self
            .public_post("/get_ticker", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data in response".into(),
        })?;

        let ticker_data = data.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("No ticker for market {market_id}"),
        })?;

        Ok(self.parse_ticker(ticker_data, symbol))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let response: CexTickerResponse = self
            .public_post("/get_ticker", HashMap::new())
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No ticker data in response".into(),
        })?;

        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for (market_id, ticker_data) in data {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                if let Some(filter_symbols) = symbols {
                    if !filter_symbols.contains(&symbol.as_str()) {
                        continue;
                    }
                }
                let ticker = self.parse_ticker(&ticker_data, symbol);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), serde_json::Value::String(market_id));

        let response: CexOrderBookResponse = self
            .public_post("/get_order_book", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order book data in response".into(),
        })?;

        let timestamp = data.timestamp.parse::<i64>().ok();

        let mut bids = Vec::new();
        if let Some(bid_array) = &data.bids {
            for bid in bid_array {
                if bid.len() >= 2 {
                    if let (Some(price), Some(amount)) = (bid[0].as_str(), bid[1].as_str()) {
                        if let (Ok(p), Ok(a)) = (price.parse::<Decimal>(), amount.parse::<Decimal>()) {
                            bids.push(OrderBookEntry { price: p, amount: a });
                        }
                    }
                }
            }
        }

        let mut asks = Vec::new();
        if let Some(ask_array) = &data.asks {
            for ask in ask_array {
                if ask.len() >= 2 {
                    if let (Some(price), Some(amount)) = (ask[0].as_str(), ask[1].as_str()) {
                        if let (Ok(p), Ok(a)) = (price.parse::<Decimal>(), amount.parse::<Decimal>()) {
                            asks.push(OrderBookEntry { price: p, amount: a });
                        }
                    }
                }
            }
        }

        Ok(OrderBook {
            symbol: symbol.to_string(),
            bids,
            asks,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .map(|dt| dt.to_rfc3339())
            }),
            nonce: None,
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("pair".to_string(), serde_json::Value::String(market_id));

        let response: CexTradesResponse = self
            .public_post("/get_trade_history", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No trade data in response".into(),
        })?;

        let trades = data.trades.unwrap_or_default();
        let mut result = Vec::new();

        for trade_data in trades {
            let timestamp = chrono::DateTime::parse_from_rfc3339(&trade_data.date_iso)
                .ok()
                .map(|dt| dt.timestamp_millis());

            let side_str = match trade_data.side.to_lowercase().as_str() {
                "buy" => "buy",
                "sell" => "sell",
                _ => "buy",
            };

            let trade_id = trade_data.trade_id.clone().unwrap_or_else(|| "".to_string());

            let trade = Trade {
                id: trade_id,
                order: None,
                info: serde_json::to_value(&trade_data).unwrap_or_default(),
                timestamp,
                datetime: Some(trade_data.date_iso.clone()),
                symbol: symbol.to_string(),
                trade_type: None,
                side: Some(side_str.to_string()),
                taker_or_maker: None,
                price: trade_data.price.unwrap_or(Decimal::ZERO),
                amount: trade_data.amount.unwrap_or(Decimal::ZERO),
                cost: None,
                fee: None,
                fees: Vec::new(),
            };
            result.push(trade);
        }

        Ok(result)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let resolution = self.timeframes.get(&timeframe)
            .ok_or_else(|| CcxtError::NotSupported {
                feature: format!("Timeframe {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("pair".to_string(), serde_json::Value::String(market_id));
        params.insert("resolution".to_string(), serde_json::Value::String(resolution.clone()));
        params.insert("dataType".to_string(), serde_json::Value::String("bestBid".to_string()));
        params.insert("toISO".to_string(), serde_json::Value::String(
            chrono::Utc::now().to_rfc3339()
        ));
        params.insert("limit".to_string(), serde_json::Value::Number(serde_json::Number::from(100)));

        let response: CexOHLCVResponse = self
            .public_post("/get_candles", params)
            .await?;

        let data = response.data.unwrap_or_default();
        let mut result = Vec::new();

        for candle in data {
            result.push(OHLCV {
                timestamp: candle.timestamp.unwrap_or(0),
                open: candle.open.unwrap_or(Decimal::ZERO),
                high: candle.high.unwrap_or(Decimal::ZERO),
                low: candle.low.unwrap_or(Decimal::ZERO),
                close: candle.close.unwrap_or(Decimal::ZERO),
                volume: candle.volume.unwrap_or(Decimal::ZERO),
            });
        }

        Ok(result)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: CexBalanceResponse = self
            .private_post("/get_my_wallet_balance", HashMap::new())
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No balance data in response".into(),
        })?;

        Ok(self.parse_balance(&data.balances))
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
        params.insert("pair".to_string(), serde_json::Value::String(market_id));
        params.insert("side".to_string(), serde_json::Value::String(
            match side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            }.to_string()
        ));
        params.insert("orderType".to_string(), serde_json::Value::String(
            match order_type {
                OrderType::Limit => "limit",
                OrderType::Market => "market",
                _ => "limit",
            }.to_string()
        ));
        params.insert("amount".to_string(), serde_json::Value::String(amount.to_string()));

        if let Some(p) = price {
            params.insert("price".to_string(), serde_json::Value::String(p.to_string()));
        }

        let response: CexOrderResponse = self
            .private_post("/do_my_new_order", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order data in response".into(),
        })?;

        Ok(self.parse_order(&data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderId".to_string(), serde_json::Value::String(id.to_string()));

        let response: CexOrderResponse = self
            .private_post("/do_cancel_my_order", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No order data in response".into(),
        })?;

        Ok(self.parse_order(&data, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("orderId".to_string(), serde_json::Value::String(id.to_string()));

        let response: CexOrdersResponse = self
            .private_post("/get_my_orders", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No orders data in response".into(),
        })?;

        let orders = data.orders.unwrap_or_default();
        let order_data = orders.into_iter().next().ok_or_else(|| CcxtError::OrderNotFound {
            order_id: id.to_string(),
        })?;

        Ok(self.parse_order(&order_data, symbol))
    }

    async fn fetch_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(sym) = symbol {
            params.insert("pair".to_string(), serde_json::Value::String(self.to_market_id(sym)));
        }

        let response: CexOrdersResponse = self
            .private_post("/get_my_orders", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No orders data in response".into(),
        })?;

        let orders = data.orders.unwrap_or_default();
        let mut result = Vec::new();

        let symbol_str = symbol.unwrap_or("UNKNOWN");
        for order_data in orders {
            result.push(self.parse_order(&order_data, symbol_str));
        }

        Ok(result)
    }

    async fn fetch_open_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), serde_json::Value::String("a".to_string())); // 'a' = active
        if let Some(sym) = symbol {
            params.insert("pair".to_string(), serde_json::Value::String(self.to_market_id(sym)));
        }

        let response: CexOrdersResponse = self
            .private_post("/get_my_orders", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No orders data in response".into(),
        })?;

        let orders = data.orders.unwrap_or_default();
        let mut result = Vec::new();

        let symbol_str = symbol.unwrap_or("UNKNOWN");
        for order_data in orders {
            result.push(self.parse_order(&order_data, symbol_str));
        }

        Ok(result)
    }

    async fn fetch_closed_orders(&self, symbol: Option<&str>, _since: Option<i64>, _limit: Option<u32>) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("status".to_string(), serde_json::Value::String("d".to_string())); // 'd' = done
        if let Some(sym) = symbol {
            params.insert("pair".to_string(), serde_json::Value::String(self.to_market_id(sym)));
        }

        let response: CexOrdersResponse = self
            .private_post("/get_my_orders", params)
            .await?;

        let data = response.data.ok_or_else(|| CcxtError::ExchangeError {
            message: "No orders data in response".into(),
        })?;

        let orders = data.orders.unwrap_or_default();
        let mut result = Vec::new();

        let symbol_str = symbol.unwrap_or("UNKNOWN");
        for order_data in orders {
            result.push(self.parse_order(&order_data, symbol_str));
        }

        Ok(result)
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
        api: &str,
        _method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        let base_url = if api == "private" {
            Self::BASE_URL_PRIVATE
        } else {
            Self::BASE_URL_PUBLIC
        };

        let url = format!("{base_url}{path}");
        let mut headers = HashMap::new();

        let request_body: Option<Vec<u8>> = if api == "public" {
            headers.insert("Content-Type".into(), "application/json".into());
            let params_json = if params.is_empty() {
                "{}".to_string()
            } else {
                serde_json::to_string(&params).unwrap_or_else(|_| "{}".to_string())
            };
            Some(params_json.into_bytes())
        } else {
            // Private API - requires signature
            let api_key = self.config.api_key().unwrap_or_default();
            let secret = self.config.secret().unwrap_or_default();

            let seconds = Utc::now().timestamp().to_string();
            let body_str = body.unwrap_or("{}");

            // Create signature: HMAC-SHA256(path + seconds + body, secret)
            let auth_string = format!("{path}{seconds}{body_str}");
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(auth_string.as_bytes());
            let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

            headers.insert("Content-Type".into(), "application/json".into());
            headers.insert("X-AGGR-KEY".into(), api_key.to_string());
            headers.insert("X-AGGR-TIMESTAMP".into(), seconds);
            headers.insert("X-AGGR-SIGNATURE".into(), signature);
            Some(body_str.as_bytes().to_vec())
        };

        SignedRequest {
            url,
            method: "POST".to_string(),
            headers,
            body: request_body.and_then(|b| String::from_utf8(b).ok()),
        }
    }
}

// Response 구조체들
#[derive(Debug, Deserialize)]
struct CexPairsInfoResponse {
    ok: String,
    data: Option<Vec<CexPairInfo>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CexPairInfo {
    symbol1: String,
    symbol2: String,
    min_lot_size_s2: i32,
    min_price_increment: i32,
}

#[derive(Debug, Deserialize)]
struct CexTickerResponse {
    ok: String,
    data: Option<HashMap<String, CexTicker>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CexTicker {
    #[serde(rename = "last")]
    last: Option<Decimal>,
    high: Option<Decimal>,
    low: Option<Decimal>,
    #[serde(rename = "bestBid")]
    best_bid: Option<Decimal>,
    #[serde(rename = "bestAsk")]
    best_ask: Option<Decimal>,
    volume: Option<Decimal>,
    #[serde(rename = "quoteVolume")]
    quote_volume: Option<Decimal>,
    #[serde(rename = "priceChange")]
    price_change: Option<Decimal>,
    #[serde(rename = "priceChangePercentage")]
    price_change_percentage: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CexOrderBookResponse {
    ok: String,
    data: Option<CexOrderBookData>,
}

#[derive(Debug, Deserialize)]
struct CexOrderBookData {
    timestamp: String,
    bids: Option<Vec<Vec<serde_json::Value>>>,
    asks: Option<Vec<Vec<serde_json::Value>>>,
}

#[derive(Debug, Deserialize)]
struct CexTradesResponse {
    ok: String,
    data: Option<CexTradesData>,
}

#[derive(Debug, Deserialize)]
struct CexTradesData {
    trades: Option<Vec<CexTradeData>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CexTradeData {
    trade_id: Option<String>,
    date_iso: String,
    side: String,
    price: Option<Decimal>,
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CexOHLCVResponse {
    ok: String,
    data: Option<Vec<CexCandle>>,
}

#[derive(Debug, Deserialize)]
struct CexCandle {
    timestamp: Option<i64>,
    open: Option<Decimal>,
    high: Option<Decimal>,
    low: Option<Decimal>,
    close: Option<Decimal>,
    volume: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CexBalanceResponse {
    ok: String,
    data: Option<CexBalanceData>,
}

#[derive(Debug, Deserialize)]
struct CexBalanceData {
    balances: HashMap<String, CexBalance>,
}

#[derive(Debug, Deserialize)]
struct CexBalance {
    available: Option<Decimal>,
    orders: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct CexOrderResponse {
    ok: String,
    data: Option<CexOrder>,
}

#[derive(Debug, Deserialize)]
struct CexOrdersResponse {
    ok: String,
    data: Option<CexOrdersData>,
}

#[derive(Debug, Deserialize)]
struct CexOrdersData {
    orders: Option<Vec<CexOrder>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CexOrder {
    order_id: String,
    client_order_id: Option<String>,
    timestamp_ms: i64,
    order_status: String,
    order_type: String,
    side: String,
    price: Option<Decimal>,
    remains: Option<Decimal>,
    executed_amount: Option<Decimal>,
    stop_price: Option<Decimal>,
    time_in_force: Option<String>,
}
