//! Bithumb Exchange Implementation
//!
//! CCXT bithumb.ts를 Rust로 포팅

use async_trait::async_trait;
use chrono::Utc;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::RwLock as TokioRwLock;

use crate::client::{ExchangeConfig, HttpClient, RateLimiter};
use crate::errors::{CcxtError, CcxtResult};
use crate::types::{
    Balance, Balances, DepositAddress, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Fee,
    Market, MarketLimits, MarketPrecision, MarketType, Order, OrderBook, OrderBookEntry, OrderSide,
    OrderStatus, OrderType, SignedRequest, Ticker, Timeframe, Trade, Transaction,
    TransactionStatus, TransactionType, WsExchange, WsMessage, OHLCV,
};

use super::bithumb_ws::BithumbWs;

type HmacSha512 = Hmac<Sha512>;

/// Bithumb 거래소
pub struct Bithumb {
    config: ExchangeConfig,
    public_client: HttpClient,
    private_client: HttpClient,
    rate_limiter: RateLimiter,
    markets: RwLock<HashMap<String, Market>>,
    markets_by_id: RwLock<HashMap<String, String>>,
    features: ExchangeFeatures,
    urls: ExchangeUrls,
    timeframes: HashMap<Timeframe, String>,
    ws_client: Arc<TokioRwLock<BithumbWs>>,
}

impl Bithumb {
    const PUBLIC_URL: &'static str = "https://api.bithumb.com/public";
    const PRIVATE_URL: &'static str = "https://api.bithumb.com";
    const RATE_LIMIT_MS: u64 = 500; // 초당 2회

    /// 새 Bithumb 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let public_client = HttpClient::new(Self::PUBLIC_URL, &config)?;
        let private_client = HttpClient::new(Self::PRIVATE_URL, &config)?;
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
            fetch_my_trades: true,
            fetch_deposits: true,
            fetch_withdrawals: true,
            fetch_deposit_address: true,
            withdraw: true,
            ws: true,
            watch_ticker: true,
            watch_order_book: true,
            watch_trades: true,
            ..Default::default()
        };

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), Self::PUBLIC_URL.into());
        api_urls.insert("private".into(), Self::PRIVATE_URL.into());

        let urls = ExchangeUrls {
            logo: Some(
                "https://github.com/user-attachments/assets/c9e0eefb-4777-46b9-8f09-9d7f7c4af82d"
                    .into(),
            ),
            api: api_urls,
            www: Some("https://www.bithumb.com".into()),
            doc: vec!["https://apidocs.bithumb.com".into()],
            fees: Some("https://en.bithumb.com/customer_support/info_fee".into()),
        };

        let mut timeframes = HashMap::new();
        timeframes.insert(Timeframe::Minute1, "1m".into());
        timeframes.insert(Timeframe::Minute3, "3m".into());
        timeframes.insert(Timeframe::Minute5, "5m".into());
        timeframes.insert(Timeframe::Minute15, "10m".into());
        timeframes.insert(Timeframe::Minute30, "30m".into());
        timeframes.insert(Timeframe::Hour1, "1h".into());
        timeframes.insert(Timeframe::Hour4, "6h".into());
        timeframes.insert(Timeframe::Day1, "24h".into());

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
            ws_client: Arc::new(TokioRwLock::new(BithumbWs::new())),
        })
    }

    /// 공개 API 호출
    async fn public_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> CcxtResult<T> {
        self.rate_limiter.throttle(1.0).await;
        self.public_client.get(path, None, None).await
    }

    /// 비공개 API 호출
    async fn private_request<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
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

        let nonce = Utc::now().timestamp_micros().to_string();

        // Build form body
        let mut body_params = params.clone();
        body_params.insert("endpoint".into(), endpoint.to_string());
        let body: String = body_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        // Create signature: endpoint + "\0" + body + "\0" + nonce
        let hmac_data = format!("{endpoint}\x00{body}\x00{nonce}");
        let mut mac =
            HmacSha512::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
        mac.update(hmac_data.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let signature_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature);

        let mut headers = HashMap::new();
        headers.insert("Accept".into(), "application/json".into());
        headers.insert(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        );
        headers.insert("Api-Key".into(), api_key.to_string());
        headers.insert("Api-Sign".into(), signature_b64);
        headers.insert("Api-Nonce".into(), nonce);

        let url = endpoint.to_string();
        self.private_client
            .post_form(&url, &body_params, Some(headers))
            .await
    }

    /// 심볼 → 마켓 ID 변환 (BTC/KRW → BTC_KRW)
    fn to_market_id(&self, symbol: &str) -> String {
        symbol.replace("/", "_")
    }

    /// 마켓 ID → 심볼 변환 (BTC_KRW → BTC/KRW)
    fn to_symbol(&self, market_id: &str) -> String {
        market_id.replace("_", "/")
    }

    /// 티커 응답 파싱
    fn parse_ticker(&self, data: &BithumbTickerData, symbol: &str) -> Ticker {
        let timestamp = data
            .date
            .parse::<i64>()
            .unwrap_or_else(|_| Utc::now().timestamp_millis());

        Ticker {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            high: data.max_price.as_ref().and_then(|s| s.parse().ok()),
            low: data.min_price.as_ref().and_then(|s| s.parse().ok()),
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: data.opening_price.as_ref().and_then(|s| s.parse().ok()),
            close: data.closing_price.as_ref().and_then(|s| s.parse().ok()),
            last: data.closing_price.as_ref().and_then(|s| s.parse().ok()),
            previous_close: data
                .prev_closing_price
                .as_ref()
                .and_then(|s| s.parse().ok()),
            change: None,
            percentage: data.fluctate_rate_24h.as_ref().and_then(|s| s.parse().ok()),
            average: None,
            base_volume: data.units_traded_24h.as_ref().and_then(|s| s.parse().ok()),
            quote_volume: data
                .acc_trade_value_24h
                .as_ref()
                .and_then(|s| s.parse().ok()),
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
        }
    }

    /// OHLCV 응답 파싱
    fn parse_ohlcv(&self, data: &[serde_json::Value]) -> Option<OHLCV> {
        if data.len() < 6 {
            return None;
        }

        Some(OHLCV::new(
            data[0].as_i64().unwrap_or(0),
            Decimal::try_from(
                data[1]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
            Decimal::try_from(
                data[2]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
            Decimal::try_from(
                data[3]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
            Decimal::try_from(
                data[4]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
            Decimal::try_from(
                data[5]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<f64>()
                    .unwrap_or(0.0),
            )
            .unwrap_or_default(),
        ))
    }

    /// 호가창 응답 파싱
    fn parse_order_book(&self, data: &BithumbOrderBookData, symbol: &str) -> OrderBook {
        let timestamp = data
            .timestamp
            .parse::<i64>()
            .unwrap_or_else(|_| Utc::now().timestamp_millis());

        let bids: Vec<OrderBookEntry> = data
            .bids
            .iter()
            .filter_map(|b| {
                Some(OrderBookEntry::new(
                    b.price.parse().ok()?,
                    b.quantity.parse().ok()?,
                ))
            })
            .collect();

        let asks: Vec<OrderBookEntry> = data
            .asks
            .iter()
            .filter_map(|a| {
                Some(OrderBookEntry::new(
                    a.price.parse().ok()?,
                    a.quantity.parse().ok()?,
                ))
            })
            .collect();

        OrderBook {
            symbol: symbol.to_string(),
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            bids,
            asks,
            checksum: None,
            nonce: None,
        }
    }

    /// 체결 내역 파싱
    fn parse_trade(&self, data: &BithumbTradeData, symbol: &str) -> Trade {
        let timestamp = data
            .transaction_date
            .parse::<i64>()
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&data.transaction_date, "%Y-%m-%d %H:%M:%S")
                    .map(|dt| dt.and_utc().timestamp_millis())
            })
            .unwrap_or_else(|_| Utc::now().timestamp_millis());

        let side = match data.r#type.as_str() {
            "ask" => "sell",
            "bid" => "buy",
            _ => "unknown",
        };

        Trade {
            id: format!("{timestamp}"),
            order: None,
            timestamp: Some(timestamp),
            datetime: Some(
                chrono::DateTime::<Utc>::from_timestamp_millis(timestamp)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            ),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price: data.price.parse().unwrap_or_default(),
            amount: data.units_traded.parse().unwrap_or_default(),
            cost: Some(data.total.parse().unwrap_or_default()),
            fee: None,
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 잔고 응답 파싱
    fn parse_balance(&self, data: &HashMap<String, serde_json::Value>) -> Balances {
        let mut balances = Balances::new();

        for key in data.keys() {
            // total_btc, in_use_btc, available_btc 형태로 데이터가 옴
            if key.starts_with("total_") {
                let currency = key.strip_prefix("total_").unwrap().to_uppercase();
                let total: Option<Decimal> = data
                    .get(&format!("total_{}", currency.to_lowercase()))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok());
                let used: Option<Decimal> = data
                    .get(&format!("in_use_{}", currency.to_lowercase()))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok());
                let free: Option<Decimal> = data
                    .get(&format!("available_{}", currency.to_lowercase()))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok());

                let balance = Balance {
                    free,
                    used,
                    total,
                    debt: None,
                };
                balances.add(&currency, balance);
            }
        }

        balances
    }

    /// 주문 상태 파싱
    fn parse_order_status(&self, status: &str) -> OrderStatus {
        match status {
            "Pending" => OrderStatus::Open,
            "Completed" => OrderStatus::Closed,
            "Cancel" => OrderStatus::Canceled,
            _ => OrderStatus::Open,
        }
    }

    /// 사용자 거래 내역 파싱
    fn parse_user_trade(&self, data: &BithumbUserTransaction, symbol: &str) -> Trade {
        let timestamp = data.transfer_date.parse::<i64>().map(|t| t / 1000).ok();
        let side = if data.search == "BUY" { "buy" } else { "sell" };
        let price: Decimal = data
            .price
            .as_ref()
            .and_then(|p| p.parse().ok())
            .unwrap_or_default();
        let amount: Decimal = data
            .units
            .as_ref()
            .and_then(|u| u.parse().ok())
            .unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|f| f.parse().ok());

        Trade {
            id: data.transfer_date.clone(),
            order: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            symbol: symbol.to_string(),
            trade_type: None,
            side: Some(side.to_string()),
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: fee_amount.map(|f| Fee::new(f, "KRW".to_string())),
            fees: Vec::new(),
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 입금 내역 파싱
    fn parse_deposit(&self, data: &BithumbTransaction) -> Transaction {
        let timestamp = data
            .transfer_date
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .map(|t| t / 1000);
        let amount: Decimal = data
            .units
            .as_ref()
            .and_then(|u| u.parse().ok())
            .unwrap_or_default();

        let status = match data.state.as_deref() {
            Some("1") => TransactionStatus::Pending,
            Some("2") => TransactionStatus::Ok,
            Some("3") => TransactionStatus::Canceled,
            _ => TransactionStatus::Pending,
        };

        Transaction {
            id: data.transfer_date.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Deposit,
            currency: data.currency.clone().unwrap_or_default(),
            network: None,
            amount,
            status,
            address: data.address.clone(),
            tag: data.destination.clone(),
            txid: data.hash.clone(),
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 출금 내역 파싱
    fn parse_withdrawal(&self, data: &BithumbTransaction) -> Transaction {
        let timestamp = data
            .transfer_date
            .as_ref()
            .and_then(|t| t.parse::<i64>().ok())
            .map(|t| t / 1000);
        let amount: Decimal = data
            .units
            .as_ref()
            .and_then(|u| u.parse().ok())
            .unwrap_or_default();
        let fee_amount: Option<Decimal> = data.fee.as_ref().and_then(|f| f.parse().ok());

        let status = match data.state.as_deref() {
            Some("1") => TransactionStatus::Pending,
            Some("2") => TransactionStatus::Ok,
            Some("3") => TransactionStatus::Canceled,
            _ => TransactionStatus::Pending,
        };

        Transaction {
            id: data.transfer_date.clone().unwrap_or_default(),
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency: data.currency.clone().unwrap_or_default(),
            network: None,
            amount,
            status,
            address: data.address.clone(),
            tag: data.destination.clone(),
            txid: data.hash.clone(),
            fee: fee_amount.map(|f| Fee::new(f, data.currency.clone().unwrap_or_default())),
            internal: None,
            confirmations: None,
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    /// 주문 응답 파싱
    fn parse_order(&self, data: &BithumbOrderData, market: Option<&Market>) -> Order {
        let status = self.parse_order_status(&data.order_status);
        let order_type = if data.order_price == "0" {
            OrderType::Market
        } else {
            OrderType::Limit
        };
        let side = match data.order_type.as_str() {
            "bid" => OrderSide::Buy,
            "ask" => OrderSide::Sell,
            _ => OrderSide::Buy,
        };

        let symbol = market
            .map(|m| m.symbol.clone())
            .unwrap_or_else(|| format!("{}/{}", data.order_currency, data.payment_currency));

        let price: Option<Decimal> = data.order_price.parse().ok();
        let amount: Decimal = data.order_qty.parse().unwrap_or_default();
        let remaining: Option<Decimal> = data.units_remaining.as_ref().and_then(|s| s.parse().ok());
        let filled = remaining.map(|r| amount - r).unwrap_or(Decimal::ZERO);

        let timestamp = data
            .transaction_date
            .parse::<i64>()
            .map(|t| t / 1000) // microseconds to milliseconds
            .ok();

        Order {
            id: data.order_id.clone(),
            client_order_id: None,
            timestamp,
            datetime: timestamp.and_then(|ts| {
                chrono::DateTime::from_timestamp_millis(ts).map(|dt| dt.to_rfc3339())
            }),
            last_trade_timestamp: None,
            last_update_timestamp: None,
            status,
            symbol,
            order_type,
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
            info: serde_json::to_value(data).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Exchange for Bithumb {
    fn id(&self) -> ExchangeId {
        ExchangeId::Bithumb
    }

    fn name(&self) -> &str {
        "Bithumb"
    }

    fn version(&self) -> &str {
        "v1"
    }

    fn countries(&self) -> &[&str] {
        &["KR"]
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
        // Bithumb doesn't have a dedicated markets endpoint
        // We derive markets from the ticker ALL endpoint
        let response: BithumbResponse<HashMap<String, serde_json::Value>> =
            self.public_get("/ticker/ALL_KRW").await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        let mut markets = Vec::new();
        for (base_id, _) in data {
            if base_id == "date" {
                continue;
            }

            let symbol = format!("{base_id}/KRW");
            let market_id = format!("{base_id}_KRW");

            markets.push(Market {
                id: market_id.clone(),
                lowercase_id: Some(market_id.to_lowercase()),
                symbol: symbol.clone(),
                base: base_id.clone(),
                quote: "KRW".to_string(),
                settle: None,
                base_id: base_id.clone(),
                quote_id: "KRW".to_string(),
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
                taker: Some(Decimal::new(25, 4)),
                maker: Some(Decimal::new(25, 4)),
                contract_size: None,
                expiry: None,
                expiry_datetime: None,
                strike: None,
                option_type: None,
            underlying: None,
            underlying_id: None,
                precision: MarketPrecision::default(),
                limits: MarketLimits::default(),
                margin_modes: None,
                created: None,
                info: serde_json::Value::Null,
                tier_based: false,
                percentage: true,
            });
        }

        Ok(markets)
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        let path = format!("/ticker/{base}_{quote}");
        let response: BithumbResponse<BithumbTickerData> = self.public_get(&path).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        Ok(self.parse_ticker(&data, symbol))
    }

    async fn fetch_tickers(
        &self,
        _symbols: Option<&[&str]>,
    ) -> CcxtResult<HashMap<String, Ticker>> {
        let response: BithumbResponse<HashMap<String, serde_json::Value>> =
            self.public_get("/ticker/ALL_KRW").await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        let mut tickers = HashMap::new();
        let date = data
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        for (base_id, value) in data {
            if base_id == "date" {
                continue;
            }

            if let Ok(mut ticker_data) = serde_json::from_value::<BithumbTickerData>(value) {
                ticker_data.date = date.clone();
                let symbol = format!("{base_id}/KRW");
                let ticker = self.parse_ticker(&ticker_data, &symbol);
                tickers.insert(symbol, ticker);
            }
        }

        Ok(tickers)
    }

    async fn fetch_order_book(&self, symbol: &str, _limit: Option<u32>) -> CcxtResult<OrderBook> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        let path = format!("/orderbook/{base}_{quote}");
        let response: BithumbResponse<BithumbOrderBookData> = self.public_get(&path).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        Ok(self.parse_order_book(&data, symbol))
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        let path = format!("/transaction_history/{base}_{quote}");
        let response: BithumbResponse<Vec<BithumbTradeData>> = self.public_get(&path).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        Ok(data.iter().map(|t| self.parse_trade(t, symbol)).collect())
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        let tf_str = self
            .timeframes
            .get(&timeframe)
            .ok_or_else(|| CcxtError::BadRequest {
                message: format!("Unsupported timeframe: {timeframe:?}"),
            })?;

        let path = format!("/candlestick/{base}_{quote}_{tf_str}");
        let response: BithumbResponse<Vec<Vec<serde_json::Value>>> = self.public_get(&path).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        Ok(data.iter().filter_map(|c| self.parse_ohlcv(c)).collect())
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        let mut params = HashMap::new();
        params.insert("currency".into(), "ALL".into());

        let response: BithumbResponse<HashMap<String, serde_json::Value>> =
            self.private_request("/info/balance", params).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        Ok(self.parse_balance(&data))
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        let mut params = HashMap::new();
        params.insert("order_currency".into(), base.to_string());
        params.insert("payment_currency".into(), quote.to_string());
        params.insert("units".into(), amount.to_string());

        let endpoint = match order_type {
            OrderType::Limit => {
                let price_val = price.ok_or_else(|| CcxtError::ArgumentsRequired {
                    message: "Limit order requires price".into(),
                })?;
                params.insert("price".into(), price_val.to_string());
                let order_side = match side {
                    OrderSide::Buy => "bid",
                    OrderSide::Sell => "ask",
                };
                params.insert("type".into(), order_side.into());
                "/trade/place"
            },
            OrderType::Market => match side {
                OrderSide::Buy => "/trade/market_buy",
                OrderSide::Sell => "/trade/market_sell",
            },
            _ => {
                return Err(CcxtError::NotSupported {
                    feature: format!("Order type: {order_type:?}"),
                });
            },
        };

        let response: BithumbResponse<BithumbCreateOrderData> =
            self.private_request(endpoint, params).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let markets = self.markets.read().unwrap();
        let _market = markets.get(symbol);

        // Create a minimal order response
        Ok(Order {
            id: response.data.map(|d| d.order_id).unwrap_or_default(),
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
            price,
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
            info: serde_json::Value::Null,
        })
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        // Note: Bithumb cancel requires 'type' (bid/ask) parameter
        // We'll need to fetch the order first or require it in params
        // For now, we'll try both and see which works
        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());
        params.insert("order_currency".into(), base.to_string());
        params.insert("payment_currency".into(), quote.to_string());
        params.insert("type".into(), "bid".into()); // Default to bid, may need adjustment

        let response: BithumbResponse<serde_json::Value> =
            self.private_request("/trade/cancel", params).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        Ok(Order {
            id: id.to_string(),
            client_order_id: None,
            timestamp: None,
            datetime: None,
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
            info: serde_json::Value::Null,
        })
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        let parts: Vec<&str> = symbol.split('/').collect();
        if parts.len() != 2 {
            return Err(CcxtError::BadSymbol {
                symbol: symbol.into(),
            });
        }
        let base = parts[0];
        let quote = parts[1];

        let mut params = HashMap::new();
        params.insert("order_id".into(), id.to_string());
        params.insert("order_currency".into(), base.to_string());
        params.insert("payment_currency".into(), quote.to_string());

        let response: BithumbResponse<BithumbOrderData> =
            self.private_request("/info/order_detail", params).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        let markets = self.markets.read().unwrap();
        let market = markets.get(symbol);
        Ok(self.parse_order(&data, market))
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let (base, quote) = if let Some(s) = symbol {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() != 2 {
                return Err(CcxtError::BadSymbol { symbol: s.into() });
            }
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("ALL".to_string(), "KRW".to_string())
        };

        let mut params = HashMap::new();
        params.insert("order_currency".into(), base.clone());
        params.insert("payment_currency".into(), quote);
        if let Some(l) = limit {
            params.insert("count".into(), l.min(100).to_string());
        }

        let response: BithumbResponse<Vec<BithumbOrderData>> =
            self.private_request("/info/orders", params).await?;

        if response.status != "0000" {
            if response
                .message
                .as_ref()
                .map(|m| m.contains("존재하지 않습니다"))
                .unwrap_or(false)
            {
                return Ok(Vec::new());
            }
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let markets = self.markets.read().unwrap();

        // Filter for completed orders
        Ok(data
            .iter()
            .filter(|o| o.order_status == "Completed")
            .map(|o| {
                let order_symbol = format!("{}/{}", o.order_currency, o.payment_currency);
                let market = markets.get(&order_symbol);
                self.parse_order(o, market)
            })
            .collect())
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        let (base, quote) = if let Some(s) = symbol {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() != 2 {
                return Err(CcxtError::BadSymbol { symbol: s.into() });
            }
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("ALL".to_string(), "KRW".to_string())
        };

        let mut params = HashMap::new();
        params.insert("order_currency".into(), base.clone());
        params.insert("payment_currency".into(), quote.clone());
        if let Some(l) = limit {
            params.insert("count".into(), l.min(100).to_string());
        }

        let response: BithumbResponse<Vec<BithumbUserTransaction>> = self
            .private_request("/info/user_transactions", params)
            .await?;

        if response.status != "0000" {
            if response
                .message
                .as_ref()
                .map(|m| m.contains("존재하지 않습니다"))
                .unwrap_or(false)
            {
                return Ok(Vec::new());
            }
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();

        // Filter and parse only trade transactions (buy/sell)
        let trades: Vec<Trade> = data
            .iter()
            .filter(|t| t.search == "BUY" || t.search == "SELL")
            .map(|t| {
                let symbol_str = if let Some(s) = symbol {
                    s.to_string()
                } else {
                    format!("{base}/{quote}")
                };
                self.parse_user_trade(t, &symbol_str)
            })
            .collect();

        Ok(trades)
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let currency = code.unwrap_or("ALL").to_uppercase();

        let mut params = HashMap::new();
        params.insert("currency".into(), currency);

        let response: BithumbResponse<Vec<BithumbTransaction>> =
            self.private_request("/info/deposit", params).await?;

        if response.status != "0000" {
            if response
                .message
                .as_ref()
                .map(|m| m.contains("존재하지 않습니다"))
                .unwrap_or(false)
            {
                return Ok(Vec::new());
            }
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();

        Ok(data.iter().map(|t| self.parse_deposit(t)).collect())
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        let currency = code.unwrap_or("ALL").to_uppercase();

        let mut params = HashMap::new();
        params.insert("currency".into(), currency);

        let response: BithumbResponse<Vec<BithumbTransaction>> =
            self.private_request("/info/withdraw", params).await?;

        if response.status != "0000" {
            if response
                .message
                .as_ref()
                .map(|m| m.contains("존재하지 않습니다"))
                .unwrap_or(false)
            {
                return Ok(Vec::new());
            }
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();

        Ok(data.iter().map(|t| self.parse_withdrawal(t)).collect())
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

        let response: BithumbResponse<BithumbDepositAddress> =
            self.private_request("/info/account", params).await?;

        if response.status != "0000" {
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.ok_or_else(|| CcxtError::BadResponse {
            message: "No data in response".into(),
        })?;

        let info = serde_json::to_value(&data).unwrap_or_default();

        Ok(DepositAddress {
            currency: code.to_string(),
            address: data.wallet_address.unwrap_or_default(),
            tag: data.destination_tag,
            network: network.map(String::from),
            info,
        })
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        _since: Option<i64>,
        _limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        let (base, quote) = if let Some(s) = symbol {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() != 2 {
                return Err(CcxtError::BadSymbol { symbol: s.into() });
            }
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("ALL".to_string(), "KRW".to_string())
        };

        let mut params = HashMap::new();
        params.insert("order_currency".into(), base.clone());
        params.insert("payment_currency".into(), quote);

        let response: BithumbResponse<Vec<BithumbOrderData>> =
            self.private_request("/info/orders", params).await?;

        if response.status != "0000" {
            // Check for "no orders" message which is not an error
            if response
                .message
                .as_ref()
                .map(|m| m.contains("존재하지 않습니다"))
                .unwrap_or(false)
            {
                return Ok(Vec::new());
            }
            return Err(CcxtError::ExchangeError {
                message: response.message.unwrap_or_default(),
            });
        }

        let data = response.data.unwrap_or_default();
        let markets = self.markets.read().unwrap();

        Ok(data
            .iter()
            .map(|o| {
                let order_symbol = format!("{}/{}", o.order_currency, o.payment_currency);
                let market = markets.get(&order_symbol);
                self.parse_order(o, market)
            })
            .collect())
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
        let url = if api == "public" {
            format!("{}{}", Self::PUBLIC_URL, path)
        } else {
            format!("{}{}", Self::PRIVATE_URL, path)
        };

        let mut headers = HashMap::new();

        if api == "private" {
            let nonce = Utc::now().timestamp_micros().to_string();
            let endpoint = path;

            let query_string = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");

            let hmac_data = format!("{endpoint}\x00{query_string}\x00{nonce}");

            if let (Some(api_key), Some(secret)) = (self.config.api_key(), self.config.secret()) {
                let mut mac = HmacSha512::new_from_slice(secret.as_bytes())
                    .expect("HMAC can take key of any size");
                mac.update(hmac_data.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());
                let signature_b64 =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature);

                headers.insert("Api-Key".into(), api_key.to_string());
                headers.insert("Api-Sign".into(), signature_b64);
                headers.insert("Api-Nonce".into(), nonce);
                headers.insert(
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                );
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

    async fn watch_ticker(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ticker(symbol).await
    }

    async fn watch_tickers(
        &self,
        symbols: &[&str],
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_tickers(symbols).await
    }

    async fn watch_order_book(
        &self,
        symbol: &str,
        limit: Option<u32>,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_order_book(symbol, limit).await
    }

    async fn watch_trades(
        &self,
        symbol: &str,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_trades(symbol).await
    }

    async fn watch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
    ) -> CcxtResult<tokio::sync::mpsc::UnboundedReceiver<WsMessage>> {
        let ws = self.ws_client.read().await;
        ws.watch_ohlcv(symbol, timeframe).await
    }
}

// === Bithumb API Response Types ===

#[derive(Debug, Deserialize)]
struct BithumbResponse<T> {
    status: String,
    message: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct BithumbTickerData {
    #[serde(default)]
    opening_price: Option<String>,
    #[serde(default)]
    closing_price: Option<String>,
    #[serde(default)]
    min_price: Option<String>,
    #[serde(default)]
    max_price: Option<String>,
    #[serde(default)]
    units_traded: Option<String>,
    #[serde(default)]
    acc_trade_value: Option<String>,
    #[serde(default)]
    prev_closing_price: Option<String>,
    #[serde(default)]
    units_traded_24h: Option<String>,
    #[serde(default)]
    acc_trade_value_24h: Option<String>,
    #[serde(default)]
    fluctate_24h: Option<String>,
    #[serde(default)]
    fluctate_rate_24h: Option<String>,
    #[serde(default)]
    date: String,
}

#[derive(Debug, Deserialize)]
struct BithumbOrderBookData {
    timestamp: String,
    #[serde(default)]
    bids: Vec<BithumbOrderBookEntry>,
    #[serde(default)]
    asks: Vec<BithumbOrderBookEntry>,
}

#[derive(Debug, Deserialize)]
struct BithumbOrderBookEntry {
    price: String,
    quantity: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbTradeData {
    transaction_date: String,
    #[serde(rename = "type")]
    r#type: String,
    units_traded: String,
    price: String,
    total: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct BithumbOrderData {
    #[serde(default)]
    order_id: String,
    #[serde(default)]
    transaction_date: String,
    #[serde(default, rename = "type")]
    order_type: String,
    #[serde(default)]
    order_status: String,
    #[serde(default)]
    order_currency: String,
    #[serde(default)]
    payment_currency: String,
    #[serde(default)]
    order_price: String,
    #[serde(default)]
    order_qty: String,
    #[serde(default)]
    units_remaining: Option<String>,
    #[serde(default)]
    cancel_date: Option<String>,
    #[serde(default)]
    cancel_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BithumbCreateOrderData {
    order_id: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BithumbUserTransaction {
    #[serde(default)]
    transfer_date: String,
    #[serde(default)]
    search: String,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    order_balance: Option<String>,
    #[serde(default)]
    payment_balance: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BithumbTransaction {
    #[serde(default)]
    transfer_date: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    fee: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    hash: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BithumbDepositAddress {
    #[serde(default)]
    wallet_address: Option<String>,
    #[serde(default)]
    destination_tag: Option<String>,
    #[serde(default)]
    currency: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_conversion() {
        let config = ExchangeConfig::new();
        let bithumb = Bithumb::new(config).unwrap();

        assert_eq!(bithumb.to_market_id("BTC/KRW"), "BTC_KRW");
        assert_eq!(bithumb.to_symbol("BTC_KRW"), "BTC/KRW");
    }

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::new();
        let bithumb = Bithumb::new(config).unwrap();

        assert_eq!(bithumb.id(), ExchangeId::Bithumb);
        assert_eq!(bithumb.name(), "Bithumb");
        assert!(bithumb.has().spot);
        assert!(!bithumb.has().swap);
    }
}
