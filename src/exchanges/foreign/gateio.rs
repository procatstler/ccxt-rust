//! Gate.io Exchange Implementation
//!
//! Alias for Gate exchange (same API, different branding)

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

use super::gate::Gate;

/// Gate.io 거래소 (Gate 래퍼)
///
/// Gate.io는 Gate와 동일한 API를 사용하는 alias입니다.
pub struct Gateio(Gate);

impl Gateio {
    /// 새 Gate.io 인스턴스 생성
    pub fn new(config: ExchangeConfig) -> CcxtResult<Self> {
        Ok(Self(Gate::new(config)?))
    }
}

#[async_trait]
impl Exchange for Gateio {
    fn id(&self) -> ExchangeId {
        ExchangeId::Gateio
    }

    fn name(&self) -> &str {
        "Gate.io"
    }

    fn has(&self) -> &ExchangeFeatures {
        self.0.has()
    }

    fn urls(&self) -> &ExchangeUrls {
        self.0.urls()
    }

    fn timeframes(&self) -> &HashMap<Timeframe, String> {
        self.0.timeframes()
    }

    fn market_id(&self, symbol: &str) -> Option<String> {
        self.0.market_id(symbol)
    }

    fn symbol(&self, market_id: &str) -> Option<String> {
        self.0.symbol(market_id)
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
        self.0.sign(path, api, method, params, headers, body)
    }

    async fn load_markets(&self, reload: bool) -> CcxtResult<HashMap<String, Market>> {
        self.0.load_markets(reload).await
    }

    async fn fetch_markets(&self) -> CcxtResult<Vec<Market>> {
        self.0.fetch_markets().await
    }

    async fn fetch_currencies(&self) -> CcxtResult<HashMap<String, Currency>> {
        self.0.fetch_currencies().await
    }

    async fn fetch_ticker(&self, symbol: &str) -> CcxtResult<Ticker> {
        self.0.fetch_ticker(symbol).await
    }

    async fn fetch_tickers(&self, symbols: Option<&[&str]>) -> CcxtResult<HashMap<String, Ticker>> {
        self.0.fetch_tickers(symbols).await
    }

    async fn fetch_order_book(&self, symbol: &str, limit: Option<u32>) -> CcxtResult<OrderBook> {
        self.0.fetch_order_book(symbol, limit).await
    }

    async fn fetch_trades(
        &self,
        symbol: &str,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.0.fetch_trades(symbol, since, limit).await
    }

    async fn fetch_ohlcv(
        &self,
        symbol: &str,
        timeframe: Timeframe,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<OHLCV>> {
        self.0.fetch_ohlcv(symbol, timeframe, since, limit).await
    }

    async fn fetch_balance(&self) -> CcxtResult<Balances> {
        self.0.fetch_balance().await
    }

    async fn create_order(
        &self,
        symbol: &str,
        order_type: OrderType,
        side: OrderSide,
        amount: Decimal,
        price: Option<Decimal>,
    ) -> CcxtResult<Order> {
        self.0.create_order(symbol, order_type, side, amount, price).await
    }

    async fn cancel_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.0.cancel_order(id, symbol).await
    }

    async fn fetch_order(&self, id: &str, symbol: &str) -> CcxtResult<Order> {
        self.0.fetch_order(id, symbol).await
    }

    async fn fetch_open_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.0.fetch_open_orders(symbol, since, limit).await
    }

    async fn fetch_closed_orders(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Order>> {
        self.0.fetch_closed_orders(symbol, since, limit).await
    }

    async fn fetch_my_trades(
        &self,
        symbol: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Trade>> {
        self.0.fetch_my_trades(symbol, since, limit).await
    }

    async fn fetch_deposits(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        self.0.fetch_deposits(code, since, limit).await
    }

    async fn fetch_withdrawals(
        &self,
        code: Option<&str>,
        since: Option<i64>,
        limit: Option<u32>,
    ) -> CcxtResult<Vec<Transaction>> {
        self.0.fetch_withdrawals(code, since, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_info() {
        let config = ExchangeConfig::default();
        let exchange = Gateio::new(config).unwrap();

        assert_eq!(exchange.id(), ExchangeId::Gateio);
        assert_eq!(exchange.name(), "Gate.io");
        assert!(exchange.has().fetch_markets);
        assert!(exchange.has().fetch_ticker);
        assert!(exchange.has().create_order);
    }
}
