//! Trade type - 체결 내역

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::Fee;

/// 체결 내역
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trade {
    /// 체결 ID
    pub id: String,
    /// 주문 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 심볼
    pub symbol: String,
    /// 주문 타입 (limit/market)
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub trade_type: Option<String>,
    /// 매수/매도
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// 테이커/메이커
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taker_or_maker: Option<TakerOrMaker>,
    /// 체결 가격
    pub price: Decimal,
    /// 체결 수량
    pub amount: Decimal,
    /// 체결 금액 (price * amount)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Decimal>,
    /// 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Fee>,
    /// 수수료 목록 (여러 화폐로 지불)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fees: Vec<Fee>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

/// 테이커/메이커 구분
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakerOrMaker {
    Taker,
    Maker,
}

impl Trade {
    /// 새 Trade 생성
    pub fn new(id: String, symbol: String, price: Decimal, amount: Decimal) -> Self {
        Self {
            id,
            order: None,
            timestamp: None,
            datetime: None,
            symbol,
            trade_type: None,
            side: None,
            taker_or_maker: None,
            price,
            amount,
            cost: Some(price * amount),
            fee: None,
            fees: Vec::new(),
            info: serde_json::Value::Null,
        }
    }

    /// 타임스탬프 설정
    pub fn with_timestamp(mut self, ts: i64) -> Self {
        self.timestamp = Some(ts);
        self.datetime = Some(
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ts)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        self
    }

    /// 매수/매도 설정
    pub fn with_side(mut self, side: &str) -> Self {
        self.side = Some(side.to_string());
        self
    }

    /// 수수료 설정
    pub fn with_fee(mut self, fee: Fee) -> Self {
        self.fee = Some(fee);
        self
    }

    /// 총 비용 계산
    pub fn calculate_cost(&self) -> Decimal {
        self.price * self.amount
    }

    /// 매수인지 확인
    pub fn is_buy(&self) -> bool {
        self.side.as_deref() == Some("buy")
    }

    /// 매도인지 확인
    pub fn is_sell(&self) -> bool {
        self.side.as_deref() == Some("sell")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_trade() {
        let trade = Trade::new(
            "12345".into(),
            "BTC/KRW".into(),
            dec!(50000000),
            dec!(0.1),
        )
        .with_side("buy")
        .with_timestamp(1700000000000);

        assert_eq!(trade.calculate_cost(), dec!(5000000));
        assert!(trade.is_buy());
        assert!(!trade.is_sell());
    }
}
