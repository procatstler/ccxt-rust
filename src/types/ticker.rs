//! Ticker type - 시세 정보

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 시세 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticker {
    /// 심볼
    pub symbol: String,
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 고가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<Decimal>,
    /// 저가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<Decimal>,
    /// 최고 매수호가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid: Option<Decimal>,
    /// 매수호가 수량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid_volume: Option<Decimal>,
    /// 최저 매도호가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Decimal>,
    /// 매도호가 수량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_volume: Option<Decimal>,
    /// 거래량 가중 평균가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vwap: Option<Decimal>,
    /// 시가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<Decimal>,
    /// 종가 (= last)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<Decimal>,
    /// 최종 거래가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<Decimal>,
    /// 전일 종가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_close: Option<Decimal>,
    /// 가격 변동
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Decimal>,
    /// 가격 변동률 (%)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<Decimal>,
    /// 평균가
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average: Option<Decimal>,
    /// 기준화폐 거래량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_volume: Option<Decimal>,
    /// 견적화폐 거래량
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_volume: Option<Decimal>,
    /// 인덱스 가격 (파생상품)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_price: Option<Decimal>,
    /// 마크 가격 (파생상품)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<Decimal>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl Ticker {
    /// 새 Ticker 생성
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            timestamp: None,
            datetime: None,
            high: None,
            low: None,
            bid: None,
            bid_volume: None,
            ask: None,
            ask_volume: None,
            vwap: None,
            open: None,
            close: None,
            last: None,
            previous_close: None,
            change: None,
            percentage: None,
            average: None,
            base_volume: None,
            quote_volume: None,
            index_price: None,
            mark_price: None,
            info: serde_json::Value::Null,
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

    /// 가격 설정
    pub fn with_prices(
        mut self,
        last: Decimal,
        bid: Option<Decimal>,
        ask: Option<Decimal>,
    ) -> Self {
        self.last = Some(last);
        self.close = Some(last);
        self.bid = bid;
        self.ask = ask;
        self
    }

    /// 변동 정보 설정
    pub fn with_change(mut self, change: Decimal, percentage: Decimal) -> Self {
        self.change = Some(change);
        self.percentage = Some(percentage);
        self
    }

    /// 거래량 설정
    pub fn with_volume(mut self, base_volume: Decimal, quote_volume: Option<Decimal>) -> Self {
        self.base_volume = Some(base_volume);
        self.quote_volume = quote_volume;
        self
    }
}

impl Default for Ticker {
    fn default() -> Self {
        Self::new(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_ticker_builder() {
        let ticker = Ticker::new("BTC/KRW".into())
            .with_timestamp(1700000000000)
            .with_prices(dec!(50000000), Some(dec!(49990000)), Some(dec!(50010000)))
            .with_change(dec!(1000000), dec!(2.04));

        assert_eq!(ticker.symbol, "BTC/KRW");
        assert_eq!(ticker.last, Some(dec!(50000000)));
        assert!(ticker.datetime.is_some());
    }
}
