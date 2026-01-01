//! MyOKX Exchange Implementation
//!
//! Alias/wrapper for OKX exchange with different hostname (eea.okx.com)
//! This is the European Economic Area version of OKX, spot trading only.

use async_trait::async_trait;
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::client::ExchangeConfig;
use crate::errors::CcxtResult;
use crate::types::{
    Balances, Currency, Exchange, ExchangeFeatures, ExchangeId, ExchangeUrls, Market,
    Order, OrderBook, OrderSide, OrderType, SignedRequest, Ticker, Timeframe, Trade,
    Transaction, OHLCV,
};

use super::okx::Okx;

/// MyOKX 거래소 (OKX 래퍼)
///
/// MyOKX는 OKX의 EEA(European Economic Area) 버전으로 spot 거래만 지원합니다.
/// - API URL: https://eea.okx.com
pub struct MyOkx {
    inner: Okx,
    urls: ExchangeUrls,
    features: ExchangeFeatures,
}

impl MyOkx {
    /// 새 MyOKX 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        let inner = Okx::new(config)?;

        let mut api_urls = HashMap::new();
        api_urls.insert("public".into(), "https://eea.okx.com".into());
        api_urls.insert("private".into(), "https://eea.okx.com".into());

        let urls = ExchangeUrls {
            logo: Some("https://user-images.githubusercontent.com/1294454/152485636-38b19e4a-bece-4dec-979a-5982859ffc04.jpg".into()),
            api: api_urls,
            www: Some("https://my.okx.com".into()),
            doc: vec![
                "https://www.okx.com/docs-v5/en/".into(),
            ],
            fees: Some("https://my.okx.com/trade-market/info/fees".into()),
        };

        // MyOKX is spot only - disable derivatives features
        let mut features = inner.has().clone();
        features.swap = false;
        features.future = false;
        features.option = false;
        features.fetch_positions = false;
        features.set_leverage = false;
        features.fetch_leverage = false;
        features.fetch_funding_rate = false;
        features.fetch_funding_rates = false;
        features.fetch_open_interest = false;
        features.fetch_liquidations = false;

        Ok(Self { inner, urls, features })
    }
}

#[async_trait]
impl Exchange for MyOkx {
    fn id(&self) -> ExchangeId {
        ExchangeId::MyOkx
    }

    fn name(&self) -> &str {
        "MyOKX"
    }

    fn has(&self) -> &ExchangeFeatures {
        &self.features
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
        // Get signed request from inner but update URL to use MyOKX API
        let mut signed = self.inner.sign(path, api, method, params, headers, body);
        signed.url = signed.url.replace("www.okx.com", "eea.okx.com");
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
        self.inner.fetch_ohlcv(symbol, timeframe, since, limit).await
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
        self.inner.create_order(symbol, order_type, side, amount, price).await
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
        let exchange = MyOkx::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::MyOkx);
        assert_eq!(exchange.name(), "MyOKX");
        assert!(exchange.has().fetch_markets);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().create_order);

        // MyOKX is spot only
        assert!(exchange.has().spot);
        assert!(!exchange.has().swap);
        assert!(!exchange.has().future);
        assert!(!exchange.has().option);

        // Check that URLs use eea.okx.com
        let urls = exchange.urls();
        assert!(urls.api.get("public").unwrap().contains("eea.okx.com"));
    }
}
