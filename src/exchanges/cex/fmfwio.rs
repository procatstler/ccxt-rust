//! FMFW.io Exchange Implementation
//!
//! Alias/wrapper for HitBTC exchange with different hostname (api.fmfw.io)

use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::client::ExchangeConfig;
use crate::errors::CcxtResult;
use crate::types::{
    Balances, Currency, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market, Order,
    OrderBook, OrderSide, OrderType, SignedRequest, Ticker, Timeframe, Trade, Transaction, OHLCV,
};

use super::hitbtc::Hitbtc;

/// FMFW.io 거래소 (HitBTC 래퍼)
///
/// FMFW.io는 HitBTC와 동일한 API를 사용하지만 다른 hostname을 사용합니다.
/// - API URL: <https://api.fmfw.io>
pub struct Fmfwio {
    inner: Hitbtc,
    urls: ExchangeUrls,
}

impl Fmfwio {
    /// 새 FMFW.io 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let inner = Hitbtc::new(config)?;

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), "https://api.fmfw.io/api/3".into());
        api_urls.insert("private".into(), "https://api.fmfw.io/api/3".into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/159177712-b685b40c-5269-4cea-ac83-f7894c49525d.jpg".into()),
            api: api_urls,
            www: Some("https://fmfw.io".into()),
            doc: vec![
                "https://api.fmfw.io/".into(),
            ],
            fees: Some("https://fmfw.io/fees-and-limits".into()),
        };

        Ok(Self { inner, urls })
    }
}

#[async_trait]
impl Exchange for Fmfwio {
    fn id(&self) -> ExchangeId {
        ExchangeId::Fmfwio
    }

    fn name(&self) -> &str {
        "FMFW.io"
    }

    fn has(&self) -> &ExchangeFeatures {
        self.inner.has()
    }

    fn urls(&self) -> &ExchangeUrls {
        &self.urls
    }

    fn timeframes(&self) -> &HashMap<Timeframe, String> {
        self.inner.timeframes()
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        self.inner.market_id(symbol)
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.inner.symbol(market_id)
    }

    fn sign(
        &self,
        path: &str,
        api: &str,
        method: &str,
        params: &HashMap<String, String>,
        headers: Option<HashMap<String, String>>,
        body: Option<&str>,
    ) -> SignedRequest {
        // Get signed request from inner but update URL to use FMFW.io API
        let mut signed = self.inner.sign(path, api, method, params, headers, body);
        signed.url = signed.url.replace("api.hitbtc.com", "api.fmfw.io");
        signed
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        self.inner.load_markets(reload).await
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        self.inner.fetch_markets().await
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>> {
        self.inner.fetch_currencies().await
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.inner.fetch_ticker(symbol).await
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.inner.fetch_tickers(symbols).await
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.inner.fetch_order_book(symbol, limit).await
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.inner.fetch_trades(symbol, since, limit).await
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.inner
            .fetch_ohlcv(symbol, timeframe, since, limit)
            .await
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        self.inner.fetch_balance().await
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        self.inner
            .create_order(symbol, order_type, side, amount, price)
            .await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.inner.cancel_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.inner.fetch_order(id, symbol).await
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.inner.fetch_open_orders(symbol, since, limit).await
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.inner.fetch_closed_orders(symbol, since, limit).await
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.inner.fetch_my_trades(symbol, since, limit).await
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        self.inner.fetch_deposits(code, since, limit).await
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        self.inner.fetch_withdrawals(code, since, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Fmfwio::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Fmfwio);
        assert_eq!(exchange.name(), "FMFW.io");
        assert!(exchange.has().fetch_markets);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().create_order);

        // Check that URLs use fmfw.io
        let urls = exchange.urls();
        assert!(urls.api.get("public").unwrap().contains("fmfw.io"));
    }
}
