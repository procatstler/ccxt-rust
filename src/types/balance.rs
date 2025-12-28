//! Balance type - 잔고 정보

use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 잔고 정보
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balances {
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 화폐별 잔고
    #[serde(flatten)]
    pub currencies: HashMap<String, Balance>,
    /// 원본 응답
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub info: serde_json::Value,
}

/// 단일 화폐 잔고
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Balance {
    /// 사용 가능 잔고
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free: Option<Decimal>,
    /// 사용 중 잔고 (주문 등)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<Decimal>,
    /// 총 잔고
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<Decimal>,
    /// 부채 (마진 거래)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debt: Option<Decimal>,
}

impl Balances {
    /// 새 Balances 생성
    pub fn new() -> Self {
        Self {
            timestamp: Some(Utc::now().timestamp_millis()),
            datetime: Some(Utc::now().to_rfc3339()),
            currencies: HashMap::new(),
            info: serde_json::Value::Null,
        }
    }

    /// 잔고 추가
    pub fn add(&mut self, currency: impl Into<String>, balance: Balance) {
        self.currencies.insert(currency.into(), balance);
    }

    /// 특정 화폐 잔고 조회
    pub fn get(&self, currency: &str) -> Option<&Balance> {
        self.currencies.get(currency)
    }

    /// 사용 가능 잔고 조회
    pub fn free(&self, currency: &str) -> Option<Decimal> {
        self.currencies.get(currency).and_then(|b| b.free)
    }

    /// 총 잔고 조회
    pub fn total(&self, currency: &str) -> Option<Decimal> {
        self.currencies.get(currency).and_then(|b| b.total)
    }

    /// 잔고가 있는 화폐 목록
    pub fn non_zero_currencies(&self) -> Vec<&String> {
        self.currencies
            .iter()
            .filter(|(_, b)| b.total.map(|t| t > Decimal::ZERO).unwrap_or(false))
            .map(|(k, _)| k)
            .collect()
    }
}

impl Balance {
    /// 새 Balance 생성
    pub fn new(free: Decimal, used: Decimal) -> Self {
        Self {
            free: Some(free),
            used: Some(used),
            total: Some(free + used),
            debt: None,
        }
    }

    /// 전체 잔고만으로 생성
    pub fn from_total(total: Decimal) -> Self {
        Self {
            free: Some(total),
            used: Some(Decimal::ZERO),
            total: Some(total),
            debt: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_balances() {
        let mut balances = Balances::new();
        balances.add("KRW", Balance::new(dec!(1000000), dec!(500000)));
        balances.add("BTC", Balance::new(dec!(0.5), dec!(0.1)));

        assert_eq!(balances.free("KRW"), Some(dec!(1000000)));
        assert_eq!(balances.total("BTC"), Some(dec!(0.6)));

        let non_zero = balances.non_zero_currencies();
        assert_eq!(non_zero.len(), 2);
    }

    #[test]
    fn test_balance() {
        let balance = Balance::new(dec!(100), dec!(20));
        assert_eq!(balance.total, Some(dec!(120)));
    }
}
