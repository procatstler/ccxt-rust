//! OrderBook type - 호가창

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 호가창
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderBook {
    /// 심볼
    pub symbol: String,
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 매수호가 (가격순 내림차순)
    pub bids: Vec<OrderBookEntry>,
    /// 매도호가 (가격순 오름차순)
    pub asks: Vec<OrderBookEntry>,
    /// 호가 시퀀스 번호
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<i64>,
}

/// 호가 항목
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookEntry {
    /// 가격
    pub price: Decimal,
    /// 수량
    pub amount: Decimal,
}

impl OrderBook {
    /// 새 OrderBook 생성
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            timestamp: None,
            datetime: None,
            bids: Vec::new(),
            asks: Vec::new(),
            nonce: None,
        }
    }

    /// 타임스탬프 설정
    pub fn with_timestamp(mut self, ts: i64) -> Self {
        self.timestamp = Some(ts);
        self.datetime = Some(
            DateTime::<Utc>::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        self
    }

    /// 최고 매수호가
    pub fn best_bid(&self) -> Option<&OrderBookEntry> {
        self.bids.first()
    }

    /// 최저 매도호가
    pub fn best_ask(&self) -> Option<&OrderBookEntry> {
        self.asks.first()
    }

    /// 스프레드 (매도-매수 가격 차이)
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }

    /// 스프레드 비율 (%)
    pub fn spread_percentage(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) if bid.price > Decimal::ZERO => {
                Some((ask.price - bid.price) / bid.price * Decimal::ONE_HUNDRED)
            }
            _ => None,
        }
    }

    /// 매수호가 추가 (정렬됨)
    pub fn add_bid(&mut self, price: Decimal, amount: Decimal) {
        let entry = OrderBookEntry { price, amount };
        let pos = self
            .bids
            .binary_search_by(|e| price.cmp(&e.price))
            .unwrap_or_else(|pos| pos);
        self.bids.insert(pos, entry);
    }

    /// 매도호가 추가 (정렬됨)
    pub fn add_ask(&mut self, price: Decimal, amount: Decimal) {
        let entry = OrderBookEntry { price, amount };
        let pos = self
            .asks
            .binary_search_by(|e| e.price.cmp(&price))
            .unwrap_or_else(|pos| pos);
        self.asks.insert(pos, entry);
    }
}

impl OrderBookEntry {
    pub fn new(price: Decimal, amount: Decimal) -> Self {
        Self { price, amount }
    }

    /// 총 가치 (price * amount)
    pub fn cost(&self) -> Decimal {
        self.price * self.amount
    }
}

// Array 형태로 변환 지원 [price, amount]
impl From<[Decimal; 2]> for OrderBookEntry {
    fn from(arr: [Decimal; 2]) -> Self {
        Self {
            price: arr[0],
            amount: arr[1],
        }
    }
}

impl From<(Decimal, Decimal)> for OrderBookEntry {
    fn from((price, amount): (Decimal, Decimal)) -> Self {
        Self { price, amount }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_orderbook() {
        let mut ob = OrderBook::new("BTC/KRW".into());
        ob.add_bid(dec!(50000000), dec!(1.0));
        ob.add_bid(dec!(49900000), dec!(2.0));
        ob.add_ask(dec!(50100000), dec!(1.5));
        ob.add_ask(dec!(50200000), dec!(0.5));

        assert_eq!(ob.best_bid().unwrap().price, dec!(50000000));
        assert_eq!(ob.best_ask().unwrap().price, dec!(50100000));
        assert_eq!(ob.spread(), Some(dec!(100000)));
    }
}
