//! Upbit Exchange Implementation
//!
//! CCXT upbit.ts를 Rust로 포팅

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::RwLock as TokioRwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry,
    OrderSide, OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade,
    Transaction, TransactionStatus, TransactionType, WsExchange, WsMessage, OHLCV,
};

use super::upbit_ws::UpbitWs;

/// Upbit 거래소
pub struct Upbit {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    ws_client: Arc<TokioRwLock<UpbitWs>>,
}

impl Upbit {
    const BASE_URL: &'static str = "https://api.upbit.com";
    const VERSION: &'static str = "v1";
    const RATE_LIMIT_MS: u64 = 50; // 초당 20회

    /// 새 Upbit 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            cors: true,
            spot: true,
            fetch_markets: true,
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
            fetch_order: true,
            fetch_open_orders: true,
            fetch_closed_orders: true,
            fetch_canceled_orders: true,
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            fetch_deposit_address: true,
            withdraw: true,
            ws: true,
            watch_ticker: true,
            watch_tickers: true,
            watch_order_book: true,
            watch_trades: true,
            watch_balance: true,
            watch_orders: true,
            watch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://user-images.githubusercontent.com/1294454/49245610-eeaabe00-f423-11e8-9cba-4b0aed794799.jpg".into(),
            ),
            api: api_urls,
            www: Some("https://upbit.com".into()),
            doc: vec![
                "https://docs.upbit.com/kr".into(),
                "https://global-docs.upbit.com".into(),
            ],
            fees: Some("https://upbit.com/service_center/guide".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Second1, "seconds".into());
        timeframes.insert(Timeframe::Minute1, "minutes/1".into());
        timeframes.insert(Timeframe::Minute3, "minutes/3".into());
        timeframes.insert(Timeframe::Minute5, "minutes/5".into());
        timeframes.insert(Timeframe::Minute15, "minutes/15".into());
        timeframes.insert(Timeframe::Minute30, "minutes/30".into());
        timeframes.insert(Timeframe::Hour1, "minutes/60".into());
        timeframes.insert(Timeframe::Hour4, "minutes/240".into());
        timeframes.insert(Timeframe::Day1, "days".into());
        timeframes.insert(Timeframe::Week1, "weeks".into());
        timeframes.insert(Timeframe::Month1, "months".into());

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes,
            ws_client: Arc::new(TokioRwLock::new(UpbitWs::new())),
        })
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(2.0).await;
        let url = format!("/{}/{}", Self::VERSION, path);
        self.client.get(&url, params, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
        body: Option<serde_json::Value>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(0.67).await;

        let signed = self.sign(path, "private", method, &params.unwrap_or_default(), None, None);

        match method {
            "GET" => {
                self.client
                    .get(&signed.url, None, Some(signed.headers))
                    .await
            }
            "POST" => {
                self.client
                    .post(&signed.url, body, Some(signed.headers))
                    .await
            }
            "DELETE" => {
                self.client
                    .delete(&signed.url, None, Some(signed.headers))
                    .await
            }
            _ => Err(CcxtError::BadRequest {
                message: format!("Unsupported method: {method}"),
            }),
        }
    }

    /// 심볼 → 마켓 ID 변환 (BTC/KRW → KRW-BTC)
    fn to_market_id(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() == 2 {
            format!("{}-{}", parts[1], parts[0])
        } else {
            symbol.to_string()
        }
    }

    /// 마켓 ID → 심볼 변환 (KRW-BTC → BTC/KRW)
    fn to_symbol(&self, market_id: &str) -> String {
        let parts: Vec<&str> = market_id.split('-').collect();
        if parts.len() == 2 {
            format!("{}/{}", parts[1], parts[0])
        } else {
            market_id.to_string()
        }
    }

    /// 마켓 응답 파싱
    fn parse_market(&self, data: &UpbitMarket) -> Market {
        let quote_id = &data.market[..3]; // KRW, BTC, USDT
        let base_id = &data.market[4..];
        let symbol = format!("{base_id}/{quote_id}");

        Market {
            id: data.market.clone(),
            lowercase_id: Some(data.market.to_lowercase()),
            symbol: symbol.clone(),
            base: base_id.to_string(),
            quote: quote_id.to_string(),
            settle: None,
            base_id: base_id.to_string(),
            quote_id: quote_id.to_string(),
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
            taker: Some(Decimal::new(25, 4)), // 0.0025 = 0.25%
            maker: Some(Decimal::new(25, 4)),
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            precision: MarketPrecision::default(),
            limits: MarketLimits::default(),
            margin_modes: None,
            created: None,
            info: serde_json::to_value(data).unwrap_or_default(),
            tier_based: false,
            percentage: true,
        }
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &UpbitTicker) -> Ticker {
        let symbol = self.to_symbol(&data.market);
        let timestamp = data
            .timestamp
            .or(data.trade_timestamp)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ticker {
            symbol,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high_price,
            low: data.low_price,
            bid: None, // Upbit doesn't provide bid/ask in ticker
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.opening_price,
            close: data.trade_price,
            last: data.trade_price,
            previous_close: data.prev_closing_price,
            change: data.signed_change_price,
            percentage: data.signed_change_rate.map(|r| r * Decimal::ONE_HUNDRED),
            average: None,
            base_volume: data.acc_trade_volume_24h.or(data.acc_trade_volume),
            quote_volume: data.acc_trade_price_24h.or(data.acc_trade_price),
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// OHLCV 응답 파싱
    fn parse_ohlcv(&self, data: &UpbitCandle) -> OHLCV {
        OHLCV::new(
            data.timestamp,
            data.opening_price,
            data.high_price,
            data.low_price,
            data.trade_price,
            data.candle_acc_trade_volume,
        )
    }

    /// 호가창 응답 파싱
    fn parse_order_book(&self, data: &UpbitOrderBook) -> OrderBook {
        let symbol = self.to_symbol(&data.market);

        let bids: Vec<OrderBookEntry> = data
            .orderbook_units
            .iter()
            .map(|u| OrderBookEntry::new(u.bid_price, u.bid_size))
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .orderbook_units
            .iter()
            .map(|u| OrderBookEntry::new(u.ask_price, u.ask_size))
            .collect();

        OrderBook {
            symbol,
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            bids,
            asks,
            nonce: None,
        }
    }

    /// 체결 내역 파싱
    fn parse_trade(&self, data: &UpbitTrade, symbol: &str) -> Trade {
        let side = match data.ask_bid.as_str() {
            "ASK" => "sell",
            "BID" => "buy",
            _ => "unknown",
        };

        Trade {
            id: data.sequential_id.to_string(),
            order: None,
            timestamp: Some(data.timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(data.timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price: data.trade_price,
            amount: data.trade_volume,
            cost: Some(data.trade_price * data.trade_volume),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 파싱
    fn parse_balance(&self, data: &[UpbitAccount]) -> Balances {
        let mut balances = Balances::new();

        for account in data {
            let balance = Balance {
                free: Some(account.balance),
                used: Some(account.locked),
                total: Some(account.balance + account.locked),
                debt: None,
            };
            balances.add(&account.currency, balance);
        }

        balances
    }

    /// 주문 파싱
    fn parse_order(&self, data: &UpbitOrder) -> Order {
        let symbol = self.to_symbol(&data.market);
        let side = match data.side.as_str() {
            "bid" => OrderSide::Buy,
            "ask" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };
        let order_type = match data.ord_type.as_str() {
            "limit" => OrderType::Limit,
            "market" | "price" => OrderType::Market,
            _ => OrderType::Limit,
        };
        let status = match data.state.as_str() {
            "wait" => OrderStatus::Open,
            "done" => OrderStatus::Closed,
            "cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.created_at)
            .map(|dt| dt.timestamp_millis())
            .ok();

        Order {
            id: data.uuid.clone(),
            client_order_id: None,
            timestamp,
            datetime: Some(data.created_at.clone()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price: data.price,
            average: data.avg_price,
            amount: data.volume,
            filled: data.executed_volume,
            remaining: data.remaining_volume,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: data.paid_fee.map(|f| data.executed_volume * data.avg_price.unwrap_or_default() + f),
            trades: Vec::new(),
            fee: data.paid_fee.map(|f| Fee::new(f, data.market[..3].to_string())),
            fees: Vec::new(),
            reduce_only: None,
            post_only: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 입출금 트랜잭션 파싱
    fn parse_transaction(&self, data: &UpbitTransaction) -> Transaction {
        let tx_type = match data.tx_type.as_str() {
            "deposit" => TransactionType::Deposit,
            "withdraw" => TransactionType::Withdrawal,
            _ => TransactionType::Deposit,
        };

        let status = match data.state.as_str() {
            "ACCEPTED" | "accepted" | "done" | "DONE" => TransactionStatus::Ok,
            "REJECTED" | "rejected" | "canceled" | "CANCELED" => TransactionStatus::Canceled,
            "PROCESSING" | "processing" | "submitting" | "submitted" | "almost_accepted" => {
                TransactionStatus::Pending
            }
            _ => TransactionStatus::Pending,
        };

        let timestamp = chrono::DateTime::parse_from_rfc3339(&data.created_at)
            .map(|dt| dt.timestamp_millis())
            .ok();

        let updated = data.done_at.as_ref().and_then(|d| {
            chrono::DateTime::parse_from_rfc3339(d)
                .map(|dt| dt.timestamp_millis())
                .ok()
        });

        Transaction {
            id: data.uuid.clone(),
            timestamp,
            datetime: Some(data.created_at.clone()),
            updated,
            tx_type,
            currency: data.currency.clone(),
            network: data.net_type.clone(),
            amount: data.amount,
            status,
            address: data.address.clone(),
            tag: data.secondary_address.clone(),
            txid: data.txid.clone(),
            fee: data.fee.map(|f| Fee::new(f, data.currency.clone())),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 입금 주소 파싱
    fn parse_deposit_address(&self, data: &UpbitDepositAddress) -> DepositAddress {
        DepositAddress {
            currency: data.currency.clone(),
            address: data.deposit_address.clone().unwrap_or_default(),
            tag: data.secondary_address.clone(),
            network: data.net_type.clone(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Upbit {
    fn id(&self) -> ExchangeId {
        ExchangeId::Upbit
    }

    fn name(&self) -> &str {
        "Upbit"
    }

    fn version(&self) -> &str {
        Self::VERSION
    }

    fn countries(&self) -> &[&str] {
        &["KR", "ID", "SG", "TH"]
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
        let response: Vec<UpbitMarket> = self.public_get("market/all", None).await?;
        Ok(response.iter().map(|m| self.parse_market(m)).collect())
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("markets".into(), market_id);

        let response: Vec<UpbitTicker> = self.public_get("ticker", Some(params)).await?;

        response
            .first()
            .map(|t| self.parse_ticker(t))
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.into(),
            })
    }

    async fn fetch_tickers(
        &self,
        symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let market_ids = match symbols {
            Some(syms) => syms.iter().map(|s| self.to_market_id(s)).collect::<Vec<_>>().join(","),
            None => {
                let markets = self.load_markets(false).await?;
                markets.keys().map(|s| self.to_market_id(s)).collect::<Vec<_>>().join(",")
            }
        };

        let mut params = HashMap::new();
        params.insert("markets".into(), market_ids);

        let response: Vec<UpbitTicker> = self.public_get("ticker", Some(params)).await?;

        Ok(response
            .iter()
            .map(|t| {
                let ticker = self.parse_ticker(t);
                (ticker.symbol.clone(), ticker)
            })
            .collect())
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("markets".into(), market_id);
        if let Some(l) = limit {
            params.insert("level".into(), l.to_string());
        }

        let response: Vec<UpbitOrderBook> = self.public_get("orderbook", Some(params)).await?;

        response
            .first()
            .map(|ob| self.parse_order_book(ob))
            .ok_or_else(|| CcxtError::BadSymbol {
                symbol: symbol.into(),
            })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("count".into(), l.min(500).to_string());
        }
        // Note: Upbit uses 'to' parameter differently, not 'since'
        let _ = since;

        let response: Vec<UpbitTrade> = self.public_get("trades/ticks", Some(params)).await?;

        Ok(response
            .iter()
            .map(|t| self.parse_trade(t, symbol))
            .collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.to_market_id(symbol);
        let tf_path = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        if let Some(l) = limit {
            params.insert("count".into(), l.min(200).to_string());
        }
        if let Some(s) = since {
            // Upbit uses 'to' parameter in ISO format
            let dt = chrono::DateTime::<Utc>::from_timestamp_millis(s);
            if let Some(dt) = dt {
                params.insert("to".into(), dt.format("%Y-%m-%dT%H:%M:%S").to_string());
            }
        }

        let path = format!("candles/{tf_path}");
        let response: Vec<UpbitCandle> = self.public_get(&path, Some(params)).await?;

        Ok(response.iter().map(|c| self.parse_ohlcv(c)).collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: Vec<UpbitAccount> = self.private_request("GET", "accounts", None, None).await?;
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

        let side_str = match side {
            OrderSide::Buy => "bid",
            OrderSide::Sell => "ask",
        };

        let ord_type = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => {
                if side == OrderSide::Buy {
                    "price" // market buy uses 'price' type in Upbit
                } else {
                    "market"
                }
            }
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                })
            }
        };

        let mut body = serde_json::json!({
            "market": market_id,
            "side": side_str,
            "ord_type": ord_type,
        });

        if order_type == OrderType::Market && side == OrderSide::Buy {
            // Market buy requires price (total KRW amount)
            let total = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Market buy order requires price (total amount)".into(),
            })?;
            body["price"] = serde_json::json!(total.to_string());
        } else {
            body["volume"] = serde_json::json!(amount.to_string());
            if let Some(p) = price {
                body["price"] = serde_json::json!(p.to_string());
            }
        }

        let mut params = HashMap::new();
        params.insert("market".into(), market_id);
        params.insert("side".into(), side_str.into());
        params.insert("ord_type".into(), ord_type.into());
        params.insert("volume".into(), amount.to_string());
        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let response: UpbitOrder = self
            .private_request("POST", "orders", Some(params), Some(body))
            .await?;

        Ok(self.parse_order(&response))
    }

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("uuid".into(), id.into());

        let response: UpbitOrder = self
            .private_request("DELETE", "order", Some(params), None)
            .await?;

        Ok(self.parse_order(&response))
    }

    async fn fetch_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let mut params = HashMap::new();
        params.insert("uuid".into(), id.into());

        let response: UpbitOrder = self
            .private_request("GET", "order", Some(params), None)
            .await?;

        Ok(self.parse_order(&response))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".into(), "wait".into());
        if let Some(s) = symbol {
            params.insert("market".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<UpbitOrder> = self
            .private_request("GET", "orders/open", Some(params), None)
            .await?;

        Ok(response.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        params.insert("state".into(), "done".into());
        if let Some(s) = symbol {
            params.insert("market".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<UpbitOrder> = self
            .private_request("GET", "orders/closed", Some(params), None)
            .await?;

        Ok(response.iter().map(|o| self.parse_order(o)).collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        // Upbit doesn't have a direct "my trades" endpoint
        // We need to fetch closed orders and convert to trades
        let mut params = HashMap::new();
        params.insert("state".into(), "done".into());
        if let Some(s) = symbol {
            params.insert("market".into(), self.to_market_id(s));
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<UpbitOrder> = self
            .private_request("GET", "orders/closed", Some(params), None)
            .await?;

        let trades: Vec<Trade> = response
            .iter()
            .filter(|o| o.executed_volume > Decimal::ZERO)
            .map(|o| {
                let symbol_str = self.to_symbol(&o.market);
                let timestamp = chrono::DateTime::parse_from_rfc3339(&o.created_at)
                    .map(|dt| dt.timestamp_millis())
                    .ok();
                let price = o.avg_price.unwrap_or_default();
                let amount = o.executed_volume;
                let cost = price * amount;

                Trade {
                    id: o.uuid.clone(),
                    order: Some(o.uuid.clone()),
                    timestamp,
                    datetime: Some(o.created_at.clone()),
                    symbol: symbol_str,
                    trade_type: None,
                    side: Some(if o.side == "bid" { "buy".to_string() } else { "sell".to_string() }),
                    taker_or_maker: None,
                    price,
                    amount,
                    cost: Some(cost),
                    fee: o.paid_fee.map(|f| Fee::new(f, o.market[..3].to_string())),
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_uppercase());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<UpbitTransaction> = self
            .private_request("GET", "deposits", Some(params), None)
            .await?;

        Ok(response.iter().map(|t| self.parse_transaction(t)).collect())
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let mut params = HashMap::new();
        if let Some(c) = code {
            params.insert("currency".into(), c.to_uppercase());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.min(100).to_string());
        }

        let response: Vec<UpbitTransaction> = self
            .private_request("GET", "withdraws", Some(params), None)
            .await?;

        Ok(response.iter().map(|t| self.parse_transaction(t)).collect())
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
        params.insert("currency".into(), code.to_uppercase());
        params.insert("amount".into(), amount.to_string());
        params.insert("address".into(), address.to_string());

        if let Some(t) = tag {
            params.insert("secondary_address".into(), t.to_string());
        }

        let net = network.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "withdraw() requires a network argument".into(),
        })?;
        params.insert("net_type".into(), net.to_string());

        let body = serde_json::json!({
            "currency": code.to_uppercase(),
            "amount": amount.to_string(),
            "address": address,
            "net_type": net,
            "secondary_address": tag.unwrap_or(""),
        });

        let response: UpbitTransaction = self
            .private_request("POST", "withdraws/coin", Some(params), Some(body))
            .await?;

        Ok(self.parse_transaction(&response))
    }

    async fn fetch_deposit_address(
        &self,
        code: &str,
        network: Option<&str>,
    ) -> CcxtResult<DepositAddress> {
        let mut params = HashMap::new();
        params.insert("currency".into(), code.to_uppercase());

        if let Some(n) = network {
            params.insert("net_type".into(), n.to_string());
        }

        let response: UpbitDepositAddress = self
            .private_request("GET", "deposits/coin_address", Some(params), None)
            .await?;

        Ok(self.parse_deposit_address(&response))
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
        method: &str,
        params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        let mut url = format!("{}/{}/{}", Self::BASE_URL, Self::VERSION, path);

        let mut headers = HashMap::new();

        if method != "POST" && !params.is_empty() {
            let query: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{url}?{query}");
        }

        if api == "private" {
            let nonce = uuid::Uuid::new_v4().to_string();

            #[derive(Serialize)]
            struct JwtPayload {
                access_key: String,
                nonce: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                query_hash: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                query_hash_alg: Option<String>,
            }

            let mut payload = JwtPayload {
                access_key: self.config.api_key().unwrap_or_default().to_string(),
                nonce,
                query_hash: None,
                query_hash_alg: None,
            };

            if !params.is_empty() {
                let query: String = params
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&");

                let mut hasher = Sha512::new();
                hasher.update(query.as_bytes());
                let hash = hex::encode(hasher.finalize());

                payload.query_hash = Some(hash);
                payload.query_hash_alg = Some("SHA512".into());
            }

            let secret = self.config.secret().unwrap_or_default();
            let token = encode(
                &Header::new(Algorithm::HS256),
                &payload,
                &EncodingKey::from_secret(secret.as_bytes()),
            )
            .unwrap_or_default();

            headers.insert("Authorization".into(), format!("Bearer {token}"));

            if method == "POST" {
                headers.insert("Content-Type".into(), "application/json".into());
            }
        }

        SignedRequest {
            url,
            method: method.to_string(),
            headers,
            body: None,
        }
    }

    // === WebSocket Methods ===

    async fn watch_ticker(&self, symbol: &str) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ticker(symbol).await
    }

    async fn watch_tickers(&self, symbols: &[&str]) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_tickers(symbols).await
    }

    async fn watch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_order_book(symbol, limit).await
    }

    async fn watch_trades(&self, symbol: &str) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_trades(symbol).await
    }

    async fn watch_ohlcv(&self, symbol: &str, timeframe: Timeframe) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ohlcv(symbol, timeframe).await
    }
}

// === Upbit API Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct UpbitMarket {
    market: String,
    korean_name: String,
    english_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTicker {
    market: String,
    trade_date: Option<String>,
    trade_time: Option<String>,
    trade_timestamp: Option<i64>,
    timestamp: Option<i64>,
    opening_price: Option<Decimal>,
    high_price: Option<Decimal>,
    low_price: Option<Decimal>,
    trade_price: Option<Decimal>,
    prev_closing_price: Option<Decimal>,
    change: Option<String>,
    change_price: Option<Decimal>,
    change_rate: Option<Decimal>,
    signed_change_price: Option<Decimal>,
    signed_change_rate: Option<Decimal>,
    trade_volume: Option<Decimal>,
    acc_trade_price: Option<Decimal>,
    acc_trade_price_24h: Option<Decimal>,
    acc_trade_volume: Option<Decimal>,
    acc_trade_volume_24h: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct UpbitCandle {
    market: String,
    candle_date_time_utc: String,
    candle_date_time_kst: String,
    opening_price: Decimal,
    high_price: Decimal,
    low_price: Decimal,
    trade_price: Decimal,
    timestamp: i64,
    candle_acc_trade_price: Decimal,
    candle_acc_trade_volume: Decimal,
}

#[derive(Debug, Deserialize)]
struct UpbitOrderBook {
    market: String,
    timestamp: i64,
    total_ask_size: Decimal,
    total_bid_size: Decimal,
    orderbook_units: Vec<UpbitOrderBookUnit>,
}

#[derive(Debug, Deserialize)]
struct UpbitOrderBookUnit {
    ask_price: Decimal,
    bid_price: Decimal,
    ask_size: Decimal,
    bid_size: Decimal,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTrade {
    market: String,
    trade_date_utc: String,
    trade_time_utc: String,
    timestamp: i64,
    trade_price: Decimal,
    trade_volume: Decimal,
    prev_closing_price: Decimal,
    change_price: Decimal,
    ask_bid: String,
    sequential_id: i64,
}

#[derive(Debug, Deserialize)]
struct UpbitAccount {
    currency: String,
    balance: Decimal,
    locked: Decimal,
    avg_buy_price: Decimal,
    avg_buy_price_modified: bool,
    unit_currency: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitOrder {
    uuid: String,
    side: String,
    ord_type: String,
    price: Option<Decimal>,
    state: String,
    market: String,
    created_at: String,
    volume: Decimal,
    remaining_volume: Option<Decimal>,
    reserved_fee: Option<Decimal>,
    remaining_fee: Option<Decimal>,
    paid_fee: Option<Decimal>,
    locked: Option<Decimal>,
    executed_volume: Decimal,
    avg_price: Option<Decimal>,
    trades_count: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitTransaction {
    #[serde(rename = "type")]
    tx_type: String,
    uuid: String,
    currency: String,
    #[serde(default)]
    net_type: Option<String>,
    txid: Option<String>,
    state: String,
    created_at: String,
    done_at: Option<String>,
    amount: Decimal,
    fee: Option<Decimal>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    secondary_address: Option<String>,
    #[serde(default)]
    transaction_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct UpbitDepositAddress {
    currency: String,
    #[serde(default)]
    net_type: Option<String>,
    deposit_address: Option<String>,
    secondary_address: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let upbit = Upbit::new(config).unwrap();

        assert_eq!(upbit.to_market_id("BTC/KRW"), "KRW-BTC");
        assert_eq!(upbit.to_market_id("ETH/BTC"), "BTC-ETH");
        assert_eq!(upbit.to_symbol("KRW-BTC"), "BTC/KRW");
        assert_eq!(upbit.to_symbol("BTC-ETH"), "ETH/BTC");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let upbit = Upbit::new(config).unwrap();

        assert_eq!(upbit.id(), ExchangeId::Upbit);
        assert_eq!(upbit.name(), "Upbit");
        assert!(upbit.has().spot);
        assert!(!upbit.has().swap);
    }
}
