//! Coincheck Exchange Implementation
//!
//! Japanese cryptocurrency exchange

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
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha256 = Hmac<Sha256>;

/// Coincheck 거래소
pub struct Coincheck {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Coincheck {
    const BASE_URL: &'static str = "https://coincheck.com/api";
    const RATE_LIMIT_MS: u64 = 200; // 5 requests per second

    /// 새 Coincheck 인스턴스 생성
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
            fetch_tickers: false,
            fetch_order_book: true,
            fetch_trades: true,
            fetch_ohlcv: false,
            fetch_balance: true,
            create_order: true,
            create_limit_order: true,
            create_market_order: true,
            cancel_order: true,
            fetch_order: true,
            fetch_orders: false,
            fetch_open_orders: true,
            fetch_my_trades: false,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::BASE_URL.into());
        api_urls.insert("private".into(), Self::BASE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/51840849/87182088-1d05fa00-c2ec-11ea-8da9-cc73b45c4010.jpg".into()),
            api: api_urls,
            www: Some("https://coincheck.com".into()),
            doc: vec!["https://coincheck.com/documents/exchange/api".into()],
            fees: Some("https://coincheck.com/info/fee".into()),
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
        let url = format!("{}{}", Self::BASE_URL, path);

        let body_str = params.as_ref()
            .map(|p| {
                p.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&")
            })
            .unwrap_or_default();

        let message = format!("{nonce}{url}{body_str}");

        let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(message.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("ACCESS-KEY".into(), api_key.to_string());
        headers.insert("ACCESS-NONCE".into(), nonce);
        headers.insert("ACCESS-SIGNATURE".into(), signature);

        match method {
            "GET" => self.client.get(path, None, Some(headers)).await,
            "POST" => {
                headers.insert("Content-Type".into(), "application/json".into());
                let body_value = params.map(|p| serde_json::to_value(p).unwrap_or(serde_json::Value::Null));
                self.client.post(path, body_value, Some(headers)).await
            }
            "DELETE" => self.client.delete(path, None, Some(headers)).await,
            _ => Err(CcxtError::NotSupported {
                feature: format!("HTTP method: {method}"),
            }),
        }
    }

    /// 마켓 ID 변환 (BTC/JPY -> btc_jpy)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }
}

#[async_trait]
impl Exchange for Coincheck {
    fn id(&self) -> ExchangeId {
        ExchangeId::Coincheck
    }

    fn name(&self) -> &str {
        "Coincheck"
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
        // Coincheck has a fixed set of JPY trading pairs
        let pairs = vec![
            ("btc_jpy", "BTC", "JPY"),
            ("etc_jpy", "ETC", "JPY"),
            ("lsk_jpy", "LSK", "JPY"),
            ("mona_jpy", "MONA", "JPY"),
            ("plt_jpy", "PLT", "JPY"),
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
                taker: Some(Decimal::ZERO),
                maker: Some(Decimal::ZERO),
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
        let path = format!("/ticker?pair={market_id}");
        let response: CoincheckTicker = self.public_get(&path).await?;

        let timestamp = response.timestamp
            .map(|t| t * 1000)
            .unwrap_or_else(|| Utc::now().timestamp_millis());

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: response.high,
            low: response.low,
            bid: response.bid,
            bid_volume: None,
            ask: response.ask,
            ask_volume: None,
            vwap: None,
            open: None,
            close: response.last,
            last: response.last,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: response.volume,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::to_value(&response).unwrap_or_default(),
        })
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/order_books?pair={market_id}");
        let response: CoincheckOrderBook = self.public_get(&path).await?;

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
            bids: parse_entries(&response.bids),
            asks: parse_entries(&response.asks),
        })
    }

    async fn fetch_trades(&self, symbol: &str, _since: Option<i64>, limit: Option<u32>) -> CcxtResult<Vec<Trade>> {
        let market_id = self.convert_to_market_id(symbol);
        let path = format!("/trades?pair={market_id}");
        let response: CoincheckTradesResponse = self.public_get(&path).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response.data.iter()
            .take(limit)
            .map(|t| {
                let timestamp = t.created_at.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                Trade {
                    id: t.id.map(|i| i.to_string()).unwrap_or_default(),
                    order: None,
                    timestamp: Some(timestamp),
                    datetime: Some(
                        chrono::DateTime::from_timestamp_millis(timestamp)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_default(),
                    ),
                    symbol: symbol.to_string(),
                    trade_type: None,
                    side: t.order_type.clone(),
                    taker_or_maker: None,
                    price: t.rate.unwrap_or_default(),
                    amount: t.amount.unwrap_or_default(),
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
        let response: CoincheckBalanceResponse = self.private_request("GET", "/accounts/balance", None).await?;

        if !response.success.unwrap_or(false) {
            return Err(CcxtError::AuthenticationError {
                message: "Failed to fetch balance".into(),
            });
        }

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        // JPY balance
        if let Some(jpy_str) = &response.jpy {
            if let Ok(jpy) = jpy_str.parse::<Decimal>() {
                let reserved = response.jpy_reserved
                    .as_ref()
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                balances.currencies.insert(
                    "JPY".to_string(),
                    Balance {
                        free: Some(jpy),
                        used: Some(reserved),
                        total: Some(jpy + reserved),
                        debt: None,
                    },
                );
            }
        }

        // BTC balance
        if let Some(btc_str) = &response.btc {
            if let Ok(btc) = btc_str.parse::<Decimal>() {
                let reserved = response.btc_reserved
                    .as_ref()
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                balances.currencies.insert(
                    "BTC".to_string(),
                    Balance {
                        free: Some(btc),
                        used: Some(reserved),
                        total: Some(btc + reserved),
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

        let order_type_str = match (side, order_type) {
            (OrderSide::Buy, OrderType::Market) => "market_buy",
            (OrderSide::Buy, _) => "buy",
            (OrderSide::Sell, OrderType::Market) => "market_sell",
            (OrderSide::Sell, _) => "sell",
        };

        let mut params = HashMap::new();
        params.insert("pair".into(), market_id);
        params.insert("order_type".into(), order_type_str.to_string());
        params.insert("amount".into(), amount.to_string());

        if order_type == OrderType::Limit {
            let price = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                message: "Price is required for limit orders".into(),
            })?;
            params.insert("rate".into(), price.to_string());
        }

        let response: CoincheckOrderResponse = self.private_request("POST", "/exchange/orders", Some(params)).await?;

        if !response.success.unwrap_or(false) {
            return Err(CcxtError::ExchangeError {
                message: response.error.unwrap_or_else(|| "Unknown error".into()),
            });
        }

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
            price,
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

    async fn cancel_order(&self, id: &str, _symbol: &str) -> CcxtResult<Order> {
        let path = format!("/exchange/orders/{id}");
        let response: CoincheckCancelResponse = self.private_request("DELETE", &path, None).await?;

        if !response.success.unwrap_or(false) {
            return Err(CcxtError::ExchangeError {
                message: "Failed to cancel order".into(),
            });
        }

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.id.map(|i| i.to_string()).unwrap_or_else(|| id.to_string()),
            client_order_id: None,
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
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
        // Coincheck doesn't have a direct fetch order endpoint
        // We need to check open orders
        let orders = self.fetch_open_orders(Some(symbol), None, None).await?;

        orders.into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| CcxtError::OrderNotFound {
                order_id: id.to_string(),
            })
    }

    async fn fetch_open_orders(
        &self,
        _symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let response: CoincheckOrdersResponse = self.private_request("GET", "/exchange/orders/opens", None).await?;

        if !response.success.unwrap_or(false) {
            return Err(CcxtError::AuthenticationError {
                message: "Failed to fetch orders".into(),
            });
        }

        let orders: Vec<Order> = response.orders.iter()
            .map(|o| {
                let timestamp = o.created_at.as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or_else(|| Utc::now().timestamp_millis());

                let side = match o.order_type.as_deref() {
                    Some("buy") | Some("limit_buy") => OrderSide::Buy,
                    Some("sell") | Some("limit_sell") => OrderSide::Sell,
                    Some("market_buy") => OrderSide::Buy,
                    Some("market_sell") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let order_type = match o.order_type.as_deref() {
                    Some(t) if t.contains("market") => OrderType::Market,
                    _ => OrderType::Limit,
                };

                let symbol = o.pair.as_ref()
                    .map(|p| {
                        let parts: Vec<&str> = p.split('_').collect();
                        if parts.len() == 2 {
                            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
                        } else {
                            p.clone()
                        }
                    })
                    .unwrap_or_default();

                let amount = o.amount.as_ref()
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

                let pending = o.pending_amount.as_ref()
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or_default();

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
                    symbol,
                    order_type,
                    time_in_force: None,
                    side,
                    price: o.rate.as_ref().and_then(|s| s.parse().ok()),
                    average: None,
                    amount,
                    filled: amount - pending,
                    remaining: Some(pending),
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
struct CoincheckTicker {
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoincheckOrderBook {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize)]
struct CoincheckTradesResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    data: Vec<CoincheckTrade>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoincheckTrade {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    rate: Option<Decimal>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoincheckOrder {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    pair: Option<String>,
    #[serde(default)]
    order_type: Option<String>,
    #[serde(default)]
    rate: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    pending_amount: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoincheckOrdersResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    orders: Vec<CoincheckOrder>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CoincheckOrderResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoincheckCancelResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CoincheckBalanceResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    jpy: Option<String>,
    #[serde(default)]
    jpy_reserved: Option<String>,
    #[serde(default)]
    btc: Option<String>,
    #[serde(default)]
    btc_reserved: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Coincheck::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Coincheck);
        assert_eq!(exchange.name(), "Coincheck");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Coincheck::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/JPY"), "btc_jpy");
    }
}
