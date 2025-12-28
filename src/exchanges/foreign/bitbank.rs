//! Bitbank Exchange Implementation
//!
//! Japanese cryptocurrency exchange

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
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// Bitbank 거래소
pub struct Bitbank {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Bitbank {
    const BASE_URL: &'static str = "https://public.bitbank.cc";
    const PRIVATE_URL: &'static str = "https://api.bitbank.cc";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// 새 Bitbank 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: false,
            swap: false,
            future: false,
            option: false,
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
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/37808081-b87f2d9c-2e59-11e8-894d-c1900b7584fe.jpg".into()),
            api: api_urls,
            www: Some("https://bitbank.cc/".into()),
            doc: vec!["https://docs.bitbank.cc/".into()],
            fees: Some("https://bitbank.cc/docs/fees/".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1min".into());
        timeframes.insert(Timeframe::Minute5, "5min".into());
        timeframes.insert(Timeframe::Minute15, "15min".into());
        timeframes.insert(Timeframe::Minute30, "30min".into());
        timeframes.insert(Timeframe::Hour1, "1hour".into());
        timeframes.insert(Timeframe::Hour4, "4hour".into());
        timeframes.insert(Timeframe::Hour8, "8hour".into());
        timeframes.insert(Timeframe::Hour12, "12hour".into());
        timeframes.insert(Timeframe::Day1, "1day".into());
        timeframes.insert(Timeframe::Week1, "1week".into());
        timeframes.insert(Timeframe::Month1, "1month".into());

        Ok(Self {
            config,
            client,
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
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, None, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = Utc::now().timestamp_millis().to_string();
        let private_client = HttpClient::new(Self::PRIVATE_URL, &self.config)?;

        let (signature_payload, body) = if method == "GET" {
            let query = if let Some(ref p) = params {
                let qs: String = p.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&");
                format!("?{qs}")
            } else {
                String::new()
            };
            (format!("{nonce}{path}{query}"), None)
        } else {
            let body = if let Some(ref p) = params {
                serde_json::to_string(p).unwrap_or_default()
            } else {
                String::new()
            };
            (format!("{nonce}{body}"), Some(serde_json::to_value(&params).unwrap_or_default()))
        };

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(signature_payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-NONCE".into(), nonce);
        headers.insert("ACCESS-SIGNATURE".into(), signature);
        headers.insert("Content-Type".into(), "application/json".into());

        match method {
            "GET" => private_client.get(path, params, Some(headers)).await,
            "POST" => private_client.post(path, body, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 응답 파싱
    fn parse_response<T>(&self, response: BitbankResponse<T>) -> CcxtResult<T> {
        if response.success == 1 {
            response.data.ok_or_else(|| CcxtError::ParseError {
                data_type: "Response".to_string(),
                message: "No data in response".into(),
            })
        } else {
            let code = response.data.as_ref()
                .and(None::<i32>)
                .unwrap_or(0);
            Err(CcxtError::ExchangeError {
                message: format!("Bitbank error: {code}"),
            })
        }
    }

    /// 심볼 변환 (BTC/JPY -> btc_jpy)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }
}

#[async_trait]
impl Exchange for Bitbank {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bitbank
    }

    fn name(&self) -> &str {
        "Bitbank"
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
        _params: &HashMap<String, String>,
        _headers: Option<HashMap<String, String>>,
        _body: Option<&str>,
    ) -> SignedRequest {
        SignedRequest {
            url: path.to_string(),
            method: method.to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        // Bitbank uses static market list
        let pairs = vec![
            ("btc_jpy", "BTC", "JPY"),
            ("xrp_jpy", "XRP", "JPY"),
            ("eth_jpy", "ETH", "JPY"),
            ("ltc_jpy", "LTC", "JPY"),
            ("mona_jpy", "MONA", "JPY"),
            ("bcc_jpy", "BCH", "JPY"),
            ("xlm_jpy", "XLM", "JPY"),
            ("bat_jpy", "BAT", "JPY"),
            ("link_jpy", "LINK", "JPY"),
            ("matic_jpy", "MATIC", "JPY"),
            ("dot_jpy", "DOT", "JPY"),
            ("doge_jpy", "DOGE", "JPY"),
            ("ada_jpy", "ADA", "JPY"),
            ("btc_usdt", "BTC", "USDT"),
            ("eth_usdt", "ETH", "USDT"),
            ("xrp_usdt", "XRP", "USDT"),
        ];

        let mut markets = Vec::new();
        for (id, base, quote) in pairs {
            let symbol = format!("{base}/{quote}");
            let market = Market {
                id: id.to_string(),
                lowercase_id: Some(id.to_string()),
                symbol: symbol.clone(),
                base: base.to_string(),
                quote: quote.to_string(),
                base_id: base.to_lowercase(),
                quote_id: quote.to_lowercase(),
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
                taker: Some(Decimal::new(12, 4)), // 0.0012
                maker: Some(Decimal::new(-2, 4)), // -0.0002 (maker rebate)
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: Some(8),
                    price: Some(0),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::json!({"id": id, "base": base, "quote": quote}),
                tier_based: false,
                percentage: true,
            };
            markets.push(market);
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
        let path = format!("/{market_id}/ticker");
        let response: BitbankResponse<BitbankTicker> = self.public_get(&path).await?;
        let data = self.parse_response(response)?;

        let timestamp = data.timestamp;
        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.high.as_ref().and_then(|s| s.parse().ok()),
            low: data.low.as_ref().and_then(|s| s.parse().ok()),
            bid: data.buy.as_ref().and_then(|s| s.parse().ok()),
            bid_volume: None,
            ask: data.sell.as_ref().and_then(|s| s.parse().ok()),
            ask_volume: None,
            vwap: None,
            open: None,
            close: data.last.as_ref().and_then(|s| s.parse().ok()),
            last: data.last.as_ref().and_then(|s| s.parse().ok()),
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: data.vol.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&data).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/{market_id}/depth");
        let response: BitbankResponse<BitbankOrderBook> = self.public_get(&path).await?;
        let data = self.parse_response(response)?;

        let timestamp = data.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());

        let parse_entries = |entries: &Vec<Vec<String>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    let price = e[0].parse::<Decimal>().ok()?;
                    let amount = e[1].parse::<Decimal>().ok()?;
                    Some(OrderBookEntry { price, amount })
                } else {
                    None
                }
            });
            if let Some(l) = limit {
                iter.take(l as usize).collect()
            } else {
                iter.collect()
            }
        };

        Ok(OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            nonce: None,
            bids: parse_entries(&data.bids),
            asks: parse_entries(&data.asks),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/{market_id}/transactions");
        let response: BitbankResponse<BitbankTransactions> = self.public_get(&path).await?;
        let data = self.parse_response(response)?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = data.transactions.iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.executed_at;
                Trade {
                    id: t.transaction_id.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.side.clone()),
                    taker_or_maker: None,
                    price: t.price.parse().unwrap_or_default(),
                    amount: t.amount.parse().unwrap_or_default(),
                    cost: None,
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(t).unwrap_or_default(),
                }
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let market_id = self.convert_to_market_id(symbol);
        let tf = self.timeframes.get(&timeframe).cloned().unwrap_or("1hour".into());
        let now = Utc::now();
        let date_str = now.format("%Y%m%d").to_string();
        let path = format!("/{market_id}/candlestick/{tf}/{date_str}");

        let response: BitbankResponse<BitbankCandlestick> = self.public_get(&path).await?;
        let data = self.parse_response(response)?;

        let limit = limit.unwrap_or(500) as usize;
        let ohlcv: Vec<OHLCV> = data.candlestick
            .first()
            .map(|cs| &cs.ohlcv)
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|c| {
                if c.len() >= 6 {
                    Some(OHLCV {
                        timestamp: c[5].as_i64()?,
                        open: c[0].as_str().and_then(|s| s.parse().ok())?,
                        high: c[1].as_str().and_then(|s| s.parse().ok())?,
                        low: c[2].as_str().and_then(|s| s.parse().ok())?,
                        close: c[3].as_str().and_then(|s| s.parse().ok())?,
                        volume: c[4].as_str().and_then(|s| s.parse().ok())?,
                    })
                } else {
                    None
                }
            })
            .take(limit)
            .collect();

        Ok(ohlcv)
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: BitbankResponse<BitbankAssets> = self
            .private_request("GET", "/v1/user/assets", None)
            .await?;
        let data = self.parse_response(response)?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        for asset in &data.assets {
            let free: Decimal = asset.free_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let used: Decimal = asset.locked_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default();
            let total = free + used;

            if let Some(currency) = &asset.asset {
                balances.currencies.insert(
                    currency.to_uppercase(),
                    Balance {
                        free: Some(free),
                        used: Some(used),
                        total: Some(total),
                        debt: None,
                    },
                );
            }
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

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("amount".into(), amount.to_string());
        params.insert("side".into(), match side {
            OrderSide::Buy => "buy".into(),
            OrderSide::Sell => "sell".into(),
        });
        params.insert("type".into(), match order_type {
            OrderType::Limit => "limit".into(),
            OrderType::Market => "market".into(),
            _ => "limit".into(),
        });

        if let Some(p) = price {
            params.insert("price".into(), p.to_string());
        }

        let response: BitbankResponse<BitbankOrder> = self
            .private_request("POST", "/v1/user/spot/order", Some(params))
            .await?;
        let data = self.parse_response(response)?;

        Ok(self.parse_order(&data, symbol))
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("order_id".into(), id.to_string());

        let response: BitbankResponse<BitbankOrder> = self
            .private_request("POST", "/v1/user/spot/cancel_order", Some(params))
            .await?;
        let data = self.parse_response(response)?;

        Ok(self.parse_order(&data, symbol))
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("order_id".into(), id.to_string());

        let response: BitbankResponse<BitbankOrder> = self
            .private_request("GET", "/v1/user/spot/order", Some(params))
            .await?;
        let data = self.parse_response(response)?;

        Ok(self.parse_order(&data, symbol))
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut all_orders = Vec::new();

        let symbols_to_fetch: Vec<String> = if let Some(s) = symbol {
            vec![s.to_string()]
        } else {
            let markets = self.markets.read().unwrap();
            markets.keys().cloned().collect()
        };

        for sym in symbols_to_fetch {
            let market_id = self.convert_to_market_id(&sym);
            let mut params = HashMap::new();
            params.insert("pair".into(), market_id);

            let response: BitbankResponse<BitbankOrders> = self
                .private_request("GET", "/v1/user/spot/active_orders", Some(params))
                .await?;

            if let Ok(data) = self.parse_response(response) {
                for order in &data.orders {
                    all_orders.push(self.parse_order(order, &sym));
                }
            }
        }

        Ok(all_orders)
    }
}

impl Bitbank {
    fn parse_order(&self, data: &BitbankOrder, symbol: &str) -> Order {
        let timestamp = data.ordered_at.unwrap_or_else(|| Utc::now().timestamp_millis());

        let status = match data.status.as_deref() {
            Some("UNFILLED") | Some("PARTIALLY_FILLED") => OrderStatus::Open,
            Some("FULLY_FILLED") => OrderStatus::Closed,
            Some("CANCELED_UNFILLED") | Some("CANCELED_PARTIALLY_FILLED") => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        };

        let side = match data.side.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let order_type = match data.order_type.as_deref() {
            Some("limit") => OrderType::Limit,
            Some("market") => OrderType::Market,
            _ => OrderType::Limit,
        };

        Order {
            id: data.order_id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: None,
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
            time_in_force: None,
            side,
            price: data.price.as_ref().and_then(|s| s.parse().ok()),
            average: data.average_price.as_ref().and_then(|s| s.parse().ok()),
            amount: data.start_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            filled: data.executed_amount.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default(),
            remaining: data.remaining_amount.as_ref().and_then(|s| s.parse().ok()),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        }
    }
}

// === Response Types ===

#[derive(Debug, Deserialize)]
struct BitbankResponse<T> {
    success: i32,
    data: Option<T>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitbankTicker {
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    last: Option<String>,
    #[serde(default)]
    buy: Option<String>,
    #[serde(default)]
    sell: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    vol: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BitbankOrderBook {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    bids: Vec<Vec<String>>,
    #[serde(default)]
    asks: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct BitbankTransactions {
    #[serde(default)]
    transactions: Vec<BitbankTransaction>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitbankTransaction {
    #[serde(default)]
    transaction_id: i64,
    #[serde(default)]
    side: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    amount: String,
    #[serde(default)]
    executed_at: i64,
}

#[derive(Debug, Deserialize)]
struct BitbankCandlestick {
    #[serde(default)]
    candlestick: Vec<BitbankCandlestickData>,
}

#[derive(Debug, Deserialize)]
struct BitbankCandlestickData {
    #[serde(default)]
    ohlcv: Vec<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct BitbankAssets {
    #[serde(default)]
    assets: Vec<BitbankAsset>,
}

#[derive(Debug, Deserialize)]
struct BitbankAsset {
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    free_amount: Option<String>,
    #[serde(default)]
    locked_amount: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BitbankOrder {
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    start_amount: Option<String>,
    #[serde(default)]
    executed_amount: Option<String>,
    #[serde(default)]
    remaining_amount: Option<String>,
    #[serde(default)]
    average_price: Option<String>,
    #[serde(default)]
    ordered_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct BitbankOrders {
    #[serde(default)]
    orders: Vec<BitbankOrder>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Bitbank::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Bitbank);
        assert_eq!(exchange.name(), "Bitbank");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Bitbank::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/JPY"), "btc_jpy");
    }
}
