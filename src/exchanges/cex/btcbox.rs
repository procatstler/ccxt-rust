//! BTCBox Exchange Implementation
//!
//! Japanese cryptocurrency exchange with limited trading pairs

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
    OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// BTCBox 거래소
pub struct Btcbox {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Btcbox {
    const BASE_URL: &'static str = "https://www.btcbox.co.jp/api/v1";
    const RATE_LIMIT_MS: u64 = 500; // 2 requests per second

    /// 새 Btcbox 인스턴스 생성
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
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: false,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: true,
            fetch_open_orders: true,
            fetch_my_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87327317-98c55400-c53c-11ea-9a11-81f7d951cc74.jpg".into()),
            api: api_urls,
            www: Some("https://www.btcbox.co.jp/".into()),
            doc: vec!["https://blog.btcbox.co.jp/en/archives/8762".into()],
            fees: Some("https://support.btcbox.co.jp/hc/ja/articles/360001235694".into()),
        };

        Ok(Self {
            config,
            client,
            rate_limiter,
            markets: RwLock::new(HashMap::new()),
            markets_by_id: RwLock::new(HashMap::new()),
            features,
            urls,
            timeframes: HashMap::new(), // No OHLCV support
        })
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.client.get(path, params, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self
            .config
            .api_key()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let api_secret = self
            .config
            .secret()
            .ok_or_else(|| CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;

        let nonce = Utc::now().timestamp_millis().to_string();

        let mut post_params = params.unwrap_or_default();
        post_params.insert("key".into(), api_key.to_string());
        post_params.insert("nonce".into(), nonce);

        let query_string: String = post_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).map_err(|_| {
            CcxtError::AuthenticationError {
                message: "Invalid secret key".into(),
            }
        })?;
        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        post_params.insert("signature".into(), signature);

        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        );

        self.client
            .post_form(path, &post_params, Some(headers))
            .await
    }

    /// 마켓 ID 변환 (BTC/JPY -> btc)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        let parts: Vec<&str> = symbol.split('/').collect();
        if !parts.is_empty() {
            parts[0].to_lowercase()
        } else {
            symbol.to_lowercase()
        }
    }
}

#[async_trait]
impl Exchange for Btcbox {
    fn id(&self) -> ExchangeId {
        ExchangeId::Btcbox
    }

    fn name(&self) -> &str {
        "BTCBox"
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
        // BTCBox has limited static market list - only JPY pairs
        let pairs = vec![
            ("btc", "BTC", "JPY"),
            ("eth", "ETH", "JPY"),
            ("ltc", "LTC", "JPY"),
            ("doge", "DOGE", "JPY"),
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
                base_id: id.to_string(),
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
                taker: Some(Decimal::new(1, 3)), // 0.001
                maker: Some(Decimal::new(1, 3)), // 0.001
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
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
        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);

        let response: BtcboxTicker = self.public_get("/ticker", Some(params)).await?;
        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: response.high,
            low: response.low,
            bid: response.buy,
            bid_volume: None,
            ask: response.sell,
            ask_volume: None,
            vwap: None,
            open: None,
            close: response.last,
            last: response.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: response.vol,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);

        let response: BtcboxOrderBook = self.public_get("/depth", Some(params)).await?;
        let timestamp = Utc::now().timestamp_millis();

        let parse_entries = |entries: &Vec<Vec<Decimal>>| -> Vec<OrderBookEntry> {
            let iter = entries.iter().filter_map(|e| {
                if e.len() >= 2 {
                    Some(OrderBookEntry {
                        price: e[0],
                        amount: e[1],
                    })
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
            datetime: Some(Utc::now().to_rfc3339()),
            nonce: None,
            checksum: None,
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);

        let response: Vec<BtcboxTrade> = self.public_get("/orders", Some(params)).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response
            .iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.date * 1000;
                Trade {
                    id: t.tid.to_string(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: Some(t.trade_type.clone()),
                    taker_or_maker: None,
                    price: t.price,
                    amount: t.amount,
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
        _symbol: &str,
        _timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        Err(CcxtError::NotSupported {
            feature: "fetch_ohlcv".to_string(),
        })
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let response: BtcboxBalance = self.private_request("/balance/", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        // JPY balance
        let jpy_free = response.jpy_balance.unwrap_or_default();
        let jpy_used = response.jpy_lock.unwrap_or_default();
        balances.currencies.insert(
            "JPY".to_string(),
            Balance {
                free: Some(jpy_free),
                used: Some(jpy_used),
                total: Some(jpy_free + jpy_used),
                debt: None,
            },
        );

        // BTC balance
        let btc_free = response.btc_balance.unwrap_or_default();
        let btc_used = response.btc_lock.unwrap_or_default();
        balances.currencies.insert(
            "BTC".to_string(),
            Balance {
                free: Some(btc_free),
                used: Some(btc_used),
                total: Some(btc_free + btc_used),
                debt: None,
            },
        );

        // ETH balance
        if let Some(eth_free) = response.eth_balance {
            let eth_used = response.eth_lock.unwrap_or_default();
            balances.currencies.insert(
                "ETH".to_string(),
                Balance {
                    free: Some(eth_free),
                    used: Some(eth_used),
                    total: Some(eth_free + eth_used),
                    debt: None,
                },
            );
        }

        // LTC balance
        if let Some(ltc_free) = response.ltc_balance {
            let ltc_used = response.ltc_lock.unwrap_or_default();
            balances.currencies.insert(
                "LTC".to_string(),
                Balance {
                    free: Some(ltc_free),
                    used: Some(ltc_used),
                    total: Some(ltc_free + ltc_used),
                    debt: None,
                },
            );
        }

        // DOGE balance
        if let Some(doge_free) = response.doge_balance {
            let doge_used = response.doge_lock.unwrap_or_default();
            balances.currencies.insert(
                "DOGE".to_string(),
                Balance {
                    free: Some(doge_free),
                    used: Some(doge_used),
                    total: Some(doge_free + doge_used),
                    debt: None,
                },
            );
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
        if order_type == OrderType::Market {
            return Err(CcxtError::NotSupported {
                feature: "Market orders".to_string(),
            });
        }

        let market_id = self.convert_to_market_id(symbol);
        let price = price.ok_or_else(|| CcxtError::ArgumentsRequired {
            message: "Price is required for limit orders".into(),
        })?;

        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);
        params.insert("amount".into(), amount.to_string());
        params.insert("price".into(), price.to_string());
        params.insert(
            "type".into(),
            match side {
                OrderSide::Buy => "buy".into(),
                OrderSide::Sell => "sell".into(),
            },
        );

        let response: BtcboxOrderResponse =
            self.private_request("/trade_add/", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.id.map(|i| i.to_string()).unwrap_or_default(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status: OrderStatus::Open,
            symbol: symbol.to_string(),
            order_type,
            time_in_force: None,
            side,
            price: Some(price),
            average: None,
            amount,
            filled: Decimal::ZERO,
            remaining: Some(amount),
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);
        params.insert("id".into(), id.to_string());

        let _response: BtcboxOrderResponse =
            self.private_request("/trade_cancel/", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
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
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);
        params.insert("id".into(), id.to_string());

        let response: BtcboxOrderDetail =
            self.private_request("/trade_view/", Some(params)).await?;

        let timestamp = response
            .datetime
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let status = if response.status == Some("cancelled".into()) {
            OrderStatus::Canceled
        } else if response.amount_original == response.amount_outstanding {
            OrderStatus::Open
        } else if response.amount_outstanding == Some(Decimal::ZERO) {
            OrderStatus::Closed
        } else {
            OrderStatus::Open
        };

        let side = match response.order_type.as_deref() {
            Some("buy") => OrderSide::Buy,
            Some("sell") => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        Ok(Order {
            id: id.to_string(),
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
            order_type: OrderType::Limit,
            time_in_force: None,
            side,
            price: response.price,
            average: None,
            amount: response.amount_original.unwrap_or_default(),
            filled: response.amount_original.unwrap_or_default()
                - response.amount_outstanding.unwrap_or_default(),
            remaining: response.amount_outstanding,
            cost: None,
            trades: Vec::new(),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(&response).unwrap_or_default(),
            stop_price: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
            reduce_only: None,
            post_only: None,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let market_id = symbol
            .map(|s| self.convert_to_market_id(s))
            .unwrap_or("btc".into());
        let symbol_str = symbol.unwrap_or("BTC/JPY");

        let mut params = HashMap::new();
        params.insert("coin".into(), market_id);
        params.insert("type".into(), "open".into());

        let response: Vec<BtcboxOrderDetail> =
            self.private_request("/trade_list/", Some(params)).await?;

        let orders: Vec<Order> = response
            .iter()
            .map(|o| {
                let timestamp = o.datetime.unwrap_or_else(|| Utc::now().timestamp_millis());
                let side = match o.order_type.as_deref() {
                    Some("buy") => OrderSide::Buy,
                    Some("sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                Order {
                    id: o.id.map(|i| i.to_string()).unwrap_or_default(),
                    client_order_id: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    last_trade_timestamp: None,
                    last_update_timestamp: None,
                    status: OrderStatus::Open,
                    symbol: symbol_str.to_string(),
                    order_type: OrderType::Limit,
                    time_in_force: None,
                    side,
                    price: o.price,
                    average: None,
                    amount: o.amount_original.unwrap_or_default(),
                    filled: o.amount_original.unwrap_or_default()
                        - o.amount_outstanding.unwrap_or_default(),
                    remaining: o.amount_outstanding,
                    cost: None,
                    trades: Vec::new(),
                    fee: None,
                    fees: Vec::new(),
                    info: serde_json::to_value(o).unwrap_or_default(),
                    stop_price: None,
                    trigger_price: None,
                    take_profit_price: None,
                    stop_loss_price: None,
                    reduce_only: None,
                    post_only: None,
                }
            })
            .collect();

        Ok(orders)
    }
}

// === Response Types ===

#[derive(Debug, Deserialize, Serialize)]
struct BtcboxTicker {
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    buy: Option<Decimal>,
    #[serde(default)]
    sell: Option<Decimal>,
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    vol: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct BtcboxOrderBook {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcboxTrade {
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    amount: Decimal,
    #[serde(default, rename = "type")]
    trade_type: String,
}

#[derive(Debug, Deserialize)]
struct BtcboxBalance {
    #[serde(default)]
    jpy_balance: Option<Decimal>,
    #[serde(default)]
    jpy_lock: Option<Decimal>,
    #[serde(default)]
    btc_balance: Option<Decimal>,
    #[serde(default)]
    btc_lock: Option<Decimal>,
    #[serde(default)]
    eth_balance: Option<Decimal>,
    #[serde(default)]
    eth_lock: Option<Decimal>,
    #[serde(default)]
    ltc_balance: Option<Decimal>,
    #[serde(default)]
    ltc_lock: Option<Decimal>,
    #[serde(default)]
    doge_balance: Option<Decimal>,
    #[serde(default)]
    doge_lock: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcboxOrderResponse {
    #[serde(default)]
    result: Option<bool>,
    #[serde(default)]
    id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BtcboxOrderDetail {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    datetime: Option<i64>,
    #[serde(default, rename = "type")]
    order_type: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    amount_original: Option<Decimal>,
    #[serde(default)]
    amount_outstanding: Option<Decimal>,
    #[serde(default)]
    status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Btcbox::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Btcbox);
        assert_eq!(exchange.name(), "BTCBox");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Btcbox::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/JPY"), "btc");
    }
}
