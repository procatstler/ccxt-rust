//! Zaif Exchange Implementation
//!
//! Japanese cryptocurrency exchange

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
    MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, OHLCV,
};

#[allow(dead_code)]
type HmacSha512 = Hmac<Sha512>;

/// Zaif 거래소
pub struct Zaif {
    config: ExchangeConfig,
    client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
}

impl Zaif {
    const BASE_URL: &'static str = "https://api.zaif.jp/api/1";
    const PRIVATE_URL: &'static str = "https://api.zaif.jp/tapi";
    const RATE_LIMIT_MS: u64 = 100; // 10 requests per second

    /// 새 Zaif 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let client = HttpClient::new(Self::BASE_URL, &config)?;
        let rate_limiter = RateLimiter::new(Self::RATE_LIMIT_MS);

        let features = ExchangeFeatures {
            spot: true,
            margin: true,
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
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/27766927-39ca2ada-5eeb-11e7-972f-1b4199518ca6.jpg".into()),
            api: api_urls,
            www: Some("https://zaif.jp/".into()),
            doc: vec![
                "https://techbureau-api-document.readthedocs.io/ja/latest/index.html".into(),
                "https://corp.zaif.jp/api-docs/".into(),
            ],
            fees: Some("https://zaif.jp/fee".into()),
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
        params: Option<HashMap<String, String>>,
    ) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;

        let api_key = self.config.api_key().ok_or_else(|| CcxtError::AuthenticationError {
            message: "API key required".into(),
        })?;
        let api_secret = self.config.secret().ok_or_else(|| CcxtError::AuthenticationError {
            message: "Secret required".into(),
        })?;

        let nonce = (Utc::now().timestamp_millis() as f64 / 1000.0).to_string();

        let mut post_params = params.unwrap_or_default();
        post_params.insert("method".into(), method.to_string());
        post_params.insert("nonce".into(), nonce);

        let query_string: String = post_params.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let mut mac = HmacSha512::new_from_slice(api_secret.as_bytes())
            .map_err(|_| CcxtError::AuthenticationError { message: "Invalid secret key".into() })?;
        mac.update(query_string.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut headers = HashMap::new();
        headers.insert("Key".into(), api_key.to_string());
        headers.insert("Sign".into(), signature);
        headers.insert("Content-Type".into(), "application/x-www-form-urlencoded".into());

        let private_client = HttpClient::new(Self::PRIVATE_URL, &self.config)?;
        let response: ZaifResponse<T> = private_client.post_form("", &post_params, Some(headers)).await?;

        if response.success == 1 {
            response.return_data.ok_or_else(|| CcxtError::ParseError {
                data_type: "Response".to_string(),
                message: "No data in response".into(),
            })
        } else {
            Err(CcxtError::ExchangeError {
                message: response.error.unwrap_or_else(|| "Unknown error".into()),
            })
        }
    }

    /// 마켓 ID 변환 (BTC/JPY -> btc_jpy)
    fn convert_to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_").to_lowercase()
    }
}

#[async_trait]
impl Exchange for Zaif {
    fn id(&self) -> ExchangeId {
        ExchangeId::Zaif
    }

    fn name(&self) -> &str {
        "Zaif"
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
        let response: HashMap<String, ZaifCurrencyPair> = self.public_get("/currency_pairs/all").await?;

        let mut markets = Vec::new();
        for (id, info) in response {
            let parts: Vec<&str> = id.split('_').collect();
            if parts.len() != 2 {
                continue;
            }

            let base = parts[0].to_uppercase();
            let quote = parts[1].to_uppercase();
            let symbol = format!("{base}/{quote}");

            let market = Market {
                id: id.clone(),
                lowercase_id: Some(id.clone()),
                symbol: symbol.clone(),
                base: base.clone(),
                quote: quote.clone(),
                base_id: parts[0].to_string(),
                quote_id: parts[1].to_string(),
                market_type: MarketType::Spot,
                spot: true,
                margin: false,
                swap: false,
                future: false,
                option: false,
                index: false,
                active: info.is_token.is_none() || !info.is_token.unwrap_or(false),
                contract: false,
                linear: None,
                inverse: None,
                sub_type: None,
                taker: None,
                maker: None,
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
                settle: None,
                settle_id: None,
                precision: MarketPrecision {
                    amount: info.item_unit_step.map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                    price: info.aux_unit_step.map(|d| {
                        let s = d.to_string();
                        if let Some(pos) = s.find('.') {
                            (s.len() - pos - 1) as i32
                        } else {
                            0
                        }
                    }),
                    cost: None,
                    base: None,
                    quote: None,
                },
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::to_value(&info).unwrap_or_default(),
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
        let path = format!("/ticker/{market_id}");
        let response: ZaifTicker = self.public_get(&path).await?;

        let timestamp = Utc::now().timestamp_millis();

        Ok(Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(Utc::now().to_rfc3339()),
            high: response.high,
            low: response.low,
            bid: response.bid,
            bid_volume: None,
            ask: response.ask,
            ask_volume: None,
            vwap: response.vwap,
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
        let path = format!("/depth/{market_id}");
        let response: ZaifOrderBook = self.public_get(&path).await?;

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
        let path = format!("/trades/{market_id}");
        let response: Vec<ZaifTrade> = self.public_get(&path).await?;

        let limit = limit.unwrap_or(100) as usize;
        let trades: Vec<Trade> = response.iter()
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
        let response: ZaifBalanceResponse = self.private_request("get_info", None).await?;

        let mut balances = Balances::default();
        let timestamp = Utc::now().timestamp_millis();
        balances.timestamp = Some(timestamp);
        balances.datetime = Some(Utc::now().to_rfc3339());

        if let Some(funds) = response.funds {
            for (currency, amount) in funds {
                let deposit = response.deposit.as_ref()
                    .and_then(|d| d.get(&currency))
                    .copied()
                    .unwrap_or_default();

                balances.currencies.insert(
                    currency.to_uppercase(),
                    Balance {
                        free: Some(amount),
                        used: Some(deposit - amount),
                        total: Some(deposit),
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
        params.insert("currency_pair".into(), market_id);
        params.insert("action".into(), match side {
            OrderSide::Buy => "bid".into(),
            OrderSide::Sell => "ask".into(),
        });
        params.insert("amount".into(), amount.to_string());
        params.insert("price".into(), price.to_string());

        let response: ZaifOrderResponse = self.private_request("trade", Some(params)).await?;

        let timestamp = Utc::now().timestamp_millis();
        Ok(Order {
            id: response.order_id.map(|i| i.to_string()).unwrap_or_default(),
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
            filled: response.received.unwrap_or_default(),
            remaining: response.remains.or(Some(amount)),
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
        let _market_id = self.convert_to_market_id(symbol);

        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());

        let _response: ZaifOrderResponse = self.private_request("cancel_order", Some(params)).await?;

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
        // Zaif doesn't have a direct fetch order endpoint
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
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("currency_pair".into(), self.convert_to_market_id(s));
        }

        let response: HashMap<String, ZaifOrderDetail> = self.private_request("active_orders", Some(params)).await?;

        let orders: Vec<Order> = response.iter()
            .map(|(id, o)| {
                let timestamp = o.timestamp.map(|t| t * 1000).unwrap_or_else(|| Utc::now().timestamp_millis());
                let side = match o.action.as_deref() {
                    Some("bid") => OrderSide::Buy,
                    Some("ask") => OrderSide::Sell,
                    _ => OrderSide::Buy,
                };

                let sym = o.currency_pair.as_ref()
                    .map(|cp| {
                        let parts: Vec<&str> = cp.split('_').collect();
                        if parts.len() == 2 {
                            format!("{}/{}", parts[0].to_uppercase(), parts[1].to_uppercase())
                        } else {
                            cp.clone()
                        }
                    })
                    .unwrap_or_else(|| symbol.unwrap_or("").to_string());

                Order {
                    id: id.clone(),
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
                    symbol: sym,
                    order_type: OrderType::Limit,
                    time_in_force: None,
                    side,
                    price: o.price,
                    average: None,
                    amount: o.amount.unwrap_or_default(),
                    filled: Decimal::ZERO,
                    remaining: o.amount,
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

#[derive(Debug, Deserialize)]
struct ZaifResponse<T> {
    #[serde(default)]
    success: i32,
    #[serde(rename = "return")]
    return_data: Option<T>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZaifCurrencyPair {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_token: Option<bool>,
    #[serde(default)]
    item_unit_step: Option<Decimal>,
    #[serde(default)]
    aux_unit_step: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZaifTicker {
    #[serde(default)]
    last: Option<Decimal>,
    #[serde(default)]
    high: Option<Decimal>,
    #[serde(default)]
    low: Option<Decimal>,
    #[serde(default)]
    vwap: Option<Decimal>,
    #[serde(default)]
    volume: Option<Decimal>,
    #[serde(default)]
    bid: Option<Decimal>,
    #[serde(default)]
    ask: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
struct ZaifOrderBook {
    #[serde(default)]
    bids: Vec<Vec<Decimal>>,
    #[serde(default)]
    asks: Vec<Vec<Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZaifTrade {
    #[serde(default)]
    tid: i64,
    #[serde(default)]
    date: i64,
    #[serde(default)]
    price: Decimal,
    #[serde(default)]
    amount: Decimal,
    #[serde(default, rename = "trade_type")]
    trade_type: String,
}

#[derive(Debug, Deserialize)]
struct ZaifBalanceResponse {
    #[serde(default)]
    funds: Option<HashMap<String, Decimal>>,
    #[serde(default)]
    deposit: Option<HashMap<String, Decimal>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZaifOrderResponse {
    #[serde(default)]
    order_id: Option<i64>,
    #[serde(default)]
    received: Option<Decimal>,
    #[serde(default)]
    remains: Option<Decimal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ZaifOrderDetail {
    #[serde(default)]
    currency_pair: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    amount: Option<Decimal>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let exchange = Zaif::new(config).unwrap();
        assert_eq!(exchange.id(), ExchangeId::Zaif);
        assert_eq!(exchange.name(), "Zaif");
        assert!(exchange.has().spot);
    }

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let exchange = Zaif::new(config).unwrap();
        assert_eq!(exchange.convert_to_market_id("BTC/JPY"), "btc_jpy");
    }
}
