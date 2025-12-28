//! Zonda Exchange Implementation
//!
//! CCXT zonda.ts를 Rust로 포팅

#![allow(dead_code)]

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
    Balance, Balances, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    MarketLimits, MarketPrecision, MarketType, MinMax, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, TakerOrMaker, Ticker, Timeframe, Trade, OHLCV,
};

type HmacSha512 = Hmac<Sha512>;

/// Zonda 거래소 (formerly BitBay)
pub struct Zonda {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    hostname: String,
}

impl Zonda {
    const HOSTNAME: &'static str = "zondacrypto.exchange";
    const RATE_LIMIT_MS: u64 = 1000; // 1 request per second

    /// 새 Zonda 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let hostname = Self::HOSTNAME.to_string();
        let public_base_url = format!("https://{}/API/Public", hostname);
        let private_base_url = format!("https://{}/API/Trading/tradingApi.php", hostname);

        let public_client = HttpClient::new(&public_base_url, &config)?;
        let private_client = HttpClient::new(&private_base_url, &config)?;
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
            fetch_ohlcv: true,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            cancel_all_orders: false,
            fetch_order: false,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_closed_orders: false,
            fetch_my_trades: true,
            fetch_deposits: false,
            fetch_withdrawals: false,
            withdraw: true,
            fetch_deposit_address: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), format!("https://{}/API/Public", hostname));
        api_urls.insert("private".into(), format!("https://{}/API/Trading/tradingApi.php", hostname));
        api_urls.insert("v1_01Public".into(), format!("https://api.{}/rest", hostname));
        api_urls.insert("v1_01Private".into(), format!("https://api.{}/rest", hostname));

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/159202310-a0e38007-5e7c-4ba9-a32f-c8263a0291fe.jpg".into()),
            api: api_urls,
            www: Some("https://zondaglobal.com".into()),
            doc: vec![
                "https://docs.zondacrypto.exchange/".into(),
                "https://github.com/BitBayNet/API".into(),
            ],
            fees: Some("https://zondaglobal.com/legal/zonda-exchange/fees".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "60".into());
        timeframes.insert(Timeframe::Minute3, "180".into());
        timeframes.insert(Timeframe::Minute5, "300".into());
        timeframes.insert(Timeframe::Minute15, "900".into());
        timeframes.insert(Timeframe::Minute30, "1800".into());
        timeframes.insert(Timeframe::Hour1, "3600".into());
        timeframes.insert(Timeframe::Hour2, "7200".into());
        timeframes.insert(Timeframe::Hour4, "14400".into());
        timeframes.insert(Timeframe::Hour6, "21600".into());
        timeframes.insert(Timeframe::Hour12, "43200".into());
        timeframes.insert(Timeframe::Day1, "86400".into());
        timeframes.insert(Timeframe::Day3, "259200".into());
        timeframes.insert(Timeframe::Week1, "604800".into());

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
            hostname,
        })
    }

    /// 공개 API 호출 (v1_01)
    async fn v1_01_public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let base_url = format!("https://api.{}/rest", self.hostname);
        let url = if let Some(p) = params {
            let query: String = p
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{base_url}{path}?{query}")
        } else {
            format!("{base_url}{path}")
        };

        let client = HttpClient::new(&base_url, &self.config)?;
        client.get(&url, None, None).await
    }

    /// 비공개 API 호출 (v1_01)
    async fn v1_01_private_request<T: serde::de::DeserializeOwned>(
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

        let base_url = format!("https://api.{}/rest", self.hostname);
        let nonce = Utc::now().timestamp_millis().to_string();

        let payload_json = if !params.is_empty() {
            serde_json::to_value(&params).unwrap_or_default()
        } else {
            serde_json::Value::Null
        };

        // Create HMAC-SHA512 signature: API-Key + nonce + payload
        let payload_string = serde_json::to_string(&payload_json).unwrap_or_default();
        let sign_string = format!("{}{}{}", api_key, nonce, payload_string);
        let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(sign_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("API-Key".into(), api_key.to_string());
        headers.insert("API-Hash".into(), signature);
        headers.insert("Request-Timestamp".into(), nonce);
        headers.insert("Operation-Id".into(), uuid::Uuid::new_v4().to_string());
        headers.insert("Content-Type".into(), "application/json".into());

        let client = HttpClient::new(&base_url, &self.config)?;
        let url = format!("{base_url}{path}");

        match method {
            "GET" => client.get(&url, None, Some(headers)).await,
            "POST" => client.post(&url, Some(payload_json), Some(headers)).await,
            "DELETE" => client.delete(&url, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 ID 변환 (BTC/USD → BTC-USD)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "-")
    }

    /// 심볼 변환 (BTC-USD → BTC/USD)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("-", "/")
    }

    /// 마켓 파싱
    fn parse_market(&self, item: &ZondaMarketItem) -> Market {
        let market_data = &item.market;
        let id = market_data.code.clone();
        let base_id = market_data.first.currency.clone();
        let quote_id = market_data.second.currency.clone();
        let base = base_id.clone();
        let quote = quote_id.clone();
        let symbol = format!("{}/{}", base, quote);

        // Determine fee tier based on fiat currencies
        let fiat_currencies = vec!["EUR", "USD", "GBP", "PLN"];
        let (maker_fee, taker_fee) = if fiat_currencies.contains(&base.as_str()) || fiat_currencies.contains(&quote.as_str()) {
            (Decimal::new(30, 4), Decimal::new(43, 4)) // 0.0030, 0.0043
        } else {
            (Decimal::ZERO, Decimal::new(1, 3)) // 0.0, 0.001
        };

        Market {
            id: id.clone(),
            lowercase_id: Some(id.to_lowercase()),
            symbol: symbol.clone(),
            base: base.clone(),
            quote: quote.clone(),
            base_id: base_id.clone(),
            quote_id: quote_id.clone(),
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
            taker: Some(taker_fee),
            maker: Some(maker_fee),
            contract_size: None,
            expiry: None,
            expiry_datetime: None,
            strike: None,
            option_type: None,
            precision: MarketPrecision {
                amount: Some(market_data.first.scale),
                price: Some(market_data.second.scale),
                cost: None,
                base: Some(market_data.first.scale),
                quote: Some(market_data.second.scale),
            },
            limits: MarketLimits {
                amount: MinMax {
                    min: market_data.first.min_offer.parse().ok(),
                    max: None,
                },
                price: MinMax {
                    min: None,
                    max: None,
                },
                cost: MinMax {
                    min: market_data.second.min_offer.parse().ok(),
                    max: None,
                },
                leverage: MinMax {
                    min: None,
                    max: None,
                },
            },
            margin_modes: None,
            created: None,
            info: serde_json::to_value(item).unwrap_or_default(),
            tier_based: false,
            percentage: true,
        }
    }

    /// 티커 파싱
    fn parse_ticker(&self, ticker: &ZondaTicker, market: Option<&Market>) -> Ticker {
        let timestamp = ticker.time.parse::<i64>().ok();
        let symbol = market.map(|m| m.symbol.clone()).unwrap_or_default();

        let last = ticker.rate.parse().ok();
        let bid = ticker.highest_bid.parse().ok();
        let ask = ticker.lowest_ask.parse().ok();
        let previous_close = ticker.previous_rate.parse().ok();

        let change = match (last, previous_close) {
            (Some(l), Some(p)) if p > Decimal::ZERO => Some(l - p),
            _ => None,
        };

        let percentage = match (last, previous_close) {
            (Some(l), Some(p)) if p > Decimal::ZERO => Some((l - p) / p * Decimal::new(100, 0)),
            _ => None,
        };

        Ticker {
            symbol,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            high: None,
            low: None,
            bid,
            bid_volume: None,
            ask,
            ask_volume: None,
            vwap: None,
            open: previous_close,
            close: last,
            last,
            previous_close,
            change,
            percentage,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(ticker).unwrap_or_default(),
        }
    }

    /// 주문 파싱
    fn parse_order(&self, order: &ZondaOrder, market: Option<&Market>) -> Order {
        let market_id = &order.market;
        let symbol = market.map(|m| m.symbol.clone()).unwrap_or_else(|| self.to_symbol(market_id));
        let timestamp = order.time.parse::<i64>().ok();

        let side = match order.offer_type.to_lowercase().as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match order.mode.as_str() {
            "limit" => OrderType::Limit,
            "market" => OrderType::Market,
            _ => OrderType::Limit,
        };

        let price = order.rate.parse().ok();
        let amount = order.start_amount.parse().unwrap_or_default();
        let remaining = order.current_amount.parse().unwrap_or_default();
        let filled = amount - remaining;

        Order {
            id: order.id.clone(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol,
            order_type,
            time_in_force: None,
            side,
            price,
            average: None,
            amount,
            filled,
            remaining: Some(remaining),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            reduce_only: None,
            post_only: order.post_only,
            info: serde_json::to_value(order).unwrap_or_default(),
        }
    }

    /// 거래 파싱
    fn parse_trade(&self, trade: &ZondaTrade, market: Option<&Market>) -> Trade {
        let market_id = &trade.market;
        let symbol = market.map(|m| m.symbol.clone()).unwrap_or_else(|| self.to_symbol(market_id));
        let timestamp = trade.time.parse::<i64>().ok();

        let price = trade.rate.parse().unwrap_or_default();
        let amount = trade.amount.parse().unwrap_or_default();
        let cost = price * amount;

        let side = match trade.user_action.to_lowercase().as_str() {
            "buy" => Some("buy".into()),
            "sell" => Some("sell".into()),
            _ => None,
        };

        let taker_or_maker = if trade.was_taker {
            Some(TakerOrMaker::Taker)
        } else {
            Some(TakerOrMaker::Maker)
        };

        let fee = trade.commission_value.as_ref().and_then(|c| {
            c.parse::<Decimal>().ok().map(|cost| crate::types::Fee {
                cost: Some(cost),
                currency: None,
                rate: None,
            })
        });

        Trade {
            id: trade.id.clone(),
            order: Some(trade.offer_id.clone()),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            symbol,
            trade_type: None,
            side,
            taker_or_maker,
            price,
            amount,
            cost: Some(cost),
            fee,
            fees: Vec::new(),
            info: serde_json::to_value(trade).unwrap_or_default(),
        }
    }

    /// 잔고 파싱
    fn parse_balance(&self, response: &ZondaBalanceResponse) -> Balances {
        let mut result = Balances::new();

        for balance_item in &response.balances {
            let code = balance_item.currency.clone();
            let free = balance_item.available_funds.parse().ok();
            let used = balance_item.locked_funds.parse().ok();
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
            result.add(&code, balance);
        }

        result
    }
}

#[async_trait]
impl Exchange for Zonda {
    fn id(&self) -> ExchangeId {
        ExchangeId::Zonda
    }

    fn name(&self) -> &str {
        "Zonda"
    }

    fn version(&self) -> &str {
        "v1.01"
    }

    fn countries(&self) -> &[&str] {
        &["EE"] // Estonia
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
        let response: ZondaTickerResponse = self
            .v1_01_public_get("/trading/ticker", None)
            .await?;

        let mut markets = Vec::new();
        for (_key, item) in response.items {
            let market = self.parse_market(&item);
            markets.push(market);
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let _ = self.load_markets(false).await?;
        let market_id = self.to_market_id(symbol);

        let response: ZondaTickerResponse = self
            .v1_01_public_get(&format!("/trading/ticker/{}", market_id), None)
            .await?;

        let ticker = response.items.get(&market_id).ok_or_else(|| CcxtError::ExchangeError {
            message: format!("Ticker not found for {}", symbol),
        })?;

        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol);

        Ok(self.parse_ticker(&ticker.ticker, market))
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        let _ = self.load_markets(false).await?;

        let response: ZondaTickerResponse = self
            .v1_01_public_get("/trading/ticker", None)
            .await?;

        let markets = self.markets.read().unwrap();
        let markets_by_id = self.markets_by_id.read().unwrap();
        let mut tickers = HashMap::new();

        for (market_id, item) in response.items {
            if let Some(symbol) = markets_by_id.get(&market_id) {
                if let Some(filter) = symbols {
                    if !filter.contains(&symbol.as_str()) {
                        continue;
                    }
                }

                let market = markets.get(symbol);
                let ticker = self.parse_ticker(&item.ticker, market);
                tickers.insert(symbol.clone(), ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let _ = self.load_markets(false).await?;
        let market_id = self.to_market_id(symbol);

        let response: ZondaOrderBookResponse = self
            .v1_01_public_get(&format!("/trading/orderbook/{}", market_id), None)
            .await?;

        let bids: Vec<OrderBookEntry> = response
            .buy
            .iter()
            .take(limit.unwrap_or(u32::MAX) as usize)
            .map(|b| OrderBookEntry {
                price: b.ra.parse().unwrap_or_default(),
                amount: b.ca.parse().unwrap_or_default(),
            })
            .collect();

        let asks: Vec<OrderBookEntry> = response
            .sell
            .iter()
            .take(limit.unwrap_or(u32::MAX) as usize)
            .map(|a| OrderBookEntry {
                price: a.ra.parse().unwrap_or_default(),
                amount: a.ca.parse().unwrap_or_default(),
            })
            .collect();

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: response.timestamp.parse().ok(),
            datetime: response.timestamp.parse::<i64>().ok().and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            nonce: response.seq_no.parse().ok(),
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
        let _ = self.load_markets(false).await?;
        let market_id = self.to_market_id(symbol);

        let mut params = HashMap::new();
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ZondaTradesResponse = self
            .v1_01_public_get(&format!("/trading/transactions/{}", market_id), Some(params))
            .await?;

        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol);

        let trades: Vec<Trade> = response
            .items
            .iter()
            .map(|t| self.parse_trade(t, market))
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
        let _ = self.load_markets(false).await?;
        let market_id = self.to_market_id(symbol);
        let resolution = self.timeframes.get(&timeframe).ok_or_else(|| CcxtError::BadRequest {
            message: format!("Unsupported timeframe: {:?}", timeframe),
        })?;

        let mut params = HashMap::new();
        if let Some(s) = since {
            params.insert("from".into(), (s / 1000).to_string());
        }
        if let Some(l) = limit {
            params.insert("limit".into(), l.to_string());
        }

        let response: ZondaOHLCVResponse = self
            .v1_01_public_get(
                &format!("/trading/candle/history/{}/{}", market_id, resolution),
                Some(params),
            )
            .await?;

        let ohlcv: Vec<OHLCV> = response
            .items
            .iter()
            .map(|c| OHLCV {
                timestamp: c.time.parse::<i64>().unwrap_or(0) * 1000,
                open: c.open.parse().unwrap_or_default(),
                high: c.high.parse().unwrap_or_default(),
                low: c.low.parse().unwrap_or_default(),
                close: c.close.parse().unwrap_or_default(),
                volume: c.volume.parse().unwrap_or_default(),
            })
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let _ = self.load_markets(false).await?;

        let response: ZondaBalanceResponse = self
            .v1_01_private_request("GET", "/balances/BITBAY/balance", HashMap::new())
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
        let _ = self.load_markets(false).await?;
        let market_id = self.to_market_id(symbol);

        let offer_type = match side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };

        let mode = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
            _ => return Err(CcxtError::NotSupported {
                feature: format!("Order type: {:?}", order_type),
            }),
        };

        let mut params = HashMap::new();
        params.insert("offerType".into(), offer_type.into());
        params.insert("mode".into(), mode.into());
        params.insert("amount".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Limit order requires price".into(),
            })?;
            params.insert("rate".into(), price_val.to_string());
        }

        let response: ZondaOrderResponse = self
            .v1_01_private_request("POST", &format!("/trading/offer/{}", market_id), params)
            .await?;

        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol);

        // Create order from response
        let order = ZondaOrder {
            id: response.offer_id.unwrap_or_default(),
            market: market_id,
            offer_type: offer_type.into(),
            mode: mode.into(),
            rate: price.map(|p| p.to_string()).unwrap_or_default(),
            start_amount: amount.to_string(),
            current_amount: amount.to_string(),
            time: Utc::now().timestamp_millis().to_string(),
            post_only: None,
        };

        Ok(self.parse_order(&order, market))
    }

    async fn cancel_order(&self, _id: &str, symbol: &str) -> CcxtResult<Order> {
        let _ = self.load_markets(false).await?;
        let _market_id = self.to_market_id(symbol);

        // Zonda requires side and price parameters for cancel
        // These should be passed via params in the original request
        // For now, we'll return an error asking for these parameters
        return Err(CcxtError::ArgumentsRequired {
            message: "Zonda cancelOrder requires 'side' and 'price' parameters".into(),
        });
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let _ = self.load_markets(false).await?;

        let response: ZondaOpenOrdersResponse = self
            .v1_01_private_request("GET", "/trading/offer", HashMap::new())
            .await?;

        let markets = self.markets.read().unwrap();

        let mut orders = Vec::new();
        for order_data in response.items {
            let market = if let Some(sym) = symbol {
                markets.get(sym)
            } else {
                let symbol_str = self.to_symbol(&order_data.market);
                markets.get(&symbol_str)
            };

            let order = self.parse_order(&order_data, market);

            // Filter by symbol if provided
            if let Some(sym) = symbol {
                if order.symbol == sym {
                    orders.push(order);
                }
            } else {
                orders.push(order);
            }
        }

        Ok(orders)
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let _ = self.load_markets(false).await?;

        let mut request: HashMap<String, String> = HashMap::new();
        if let Some(sym) = symbol {
            let market_id = self.to_market_id(sym);
            request.insert("markets".into(), serde_json::json!([market_id]).to_string());
        }

        let query = HashMap::from([
            ("query".into(), serde_json::to_string(&request).unwrap_or_default())
        ]);

        let response: ZondaMyTradesResponse = self
            .v1_01_private_request("GET", "/trading/history/transactions", query)
            .await?;

        let markets = self.markets.read().unwrap();

        let mut trades: Vec<Trade> = response
            .items
            .iter()
            .filter_map(|t| {
                if let Some(ts) = since {
                    if let Ok(trade_time) = t.time.parse::<i64>() {
                        if trade_time < ts {
                            return None;
                        }
                    }
                }

                let symbol_str = self.to_symbol(&t.market);
                let market = markets.get(&symbol_str);
                Some(self.parse_trade(t, market))
            })
            .collect();

        if let Some(l) = limit {
            trades.truncate(l as usize);
        }

        Ok(trades)
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        // Fetch all open orders and find the matching one
        let orders = self.fetch_open_orders(Some(symbol), None, None).await?;
        orders
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
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
        _path: &str,
        _api: &str,
        _method: &str,
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        // Signing is handled in the private request methods
        SignedRequest {
            url: String::new(),
            method: String::new(),
            headers: HashMap::new(),
            body: None,
        }
    }
}

// === Zonda API Response Types ===

#[derive(Debug, Deserialize)]
struct ZondaTickerResponse {
    status: String,
    items: HashMap<String, ZondaMarketItem>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaMarketItem {
    market: ZondaMarket,
    #[serde(flatten)]
    ticker: ZondaTicker,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaMarket {
    code: String,
    first: ZondaCurrencyInfo,
    second: ZondaCurrencyInfo,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ZondaCurrencyInfo {
    currency: String,
    min_offer: String,
    scale: i32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ZondaTicker {
    time: String,
    highest_bid: String,
    lowest_ask: String,
    rate: String,
    previous_rate: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZondaOrderBookResponse {
    status: String,
    buy: Vec<ZondaOrderBookEntry>,
    sell: Vec<ZondaOrderBookEntry>,
    timestamp: String,
    seq_no: String,
}

#[derive(Debug, Deserialize)]
struct ZondaOrderBookEntry {
    ra: String, // rate
    ca: String, // current amount
}

#[derive(Debug, Deserialize, Serialize)]
struct ZondaOrder {
    id: String,
    market: String,
    #[serde(rename = "offerType")]
    offer_type: String,
    mode: String,
    rate: String,
    #[serde(rename = "startAmount")]
    start_amount: String,
    #[serde(rename = "currentAmount")]
    current_amount: String,
    time: String,
    #[serde(rename = "postOnly")]
    post_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ZondaOpenOrdersResponse {
    status: String,
    items: Vec<ZondaOrder>,
}

#[derive(Debug, Deserialize)]
struct ZondaOrderResponse {
    status: String,
    #[serde(rename = "offerId")]
    offer_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ZondaTrade {
    id: String,
    market: String,
    time: String,
    amount: String,
    rate: String,
    #[serde(rename = "userAction")]
    user_action: String,
    #[serde(rename = "wasTaker")]
    was_taker: bool,
    #[serde(rename = "offerId")]
    offer_id: String,
    #[serde(rename = "commissionValue")]
    commission_value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZondaTradesResponse {
    status: String,
    items: Vec<ZondaTrade>,
}

#[derive(Debug, Deserialize)]
struct ZondaMyTradesResponse {
    status: String,
    items: Vec<ZondaTrade>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZondaBalanceResponse {
    status: String,
    balances: Vec<ZondaBalance>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZondaBalance {
    currency: String,
    available_funds: String,
    locked_funds: String,
}

#[derive(Debug, Deserialize)]
struct ZondaOHLCVResponse {
    status: String,
    items: Vec<ZondaCandle>,
}

#[derive(Debug, Deserialize)]
struct ZondaCandle {
    time: String,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let zonda = Zonda::new(config).unwrap();

        assert_eq!(zonda.to_market_id("BTC/USD"), "BTC-USD");
        assert_eq!(zonda.to_market_id("ETH/EUR"), "ETH-EUR");
        assert_eq!(zonda.to_symbol("BTC-USD"), "BTC/USD");
        assert_eq!(zonda.to_symbol("ETH-EUR"), "ETH/EUR");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let zonda = Zonda::new(config).unwrap();

        assert_eq!(zonda.id(), ExchangeId::Zonda);
        assert_eq!(zonda.name(), "Zonda");
        assert!(zonda.has().spot);
        assert!(!zonda.has().margin);
        assert!(!zonda.has().swap);
    }
}
