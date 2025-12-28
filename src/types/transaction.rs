//! Transaction type - 입출금 내역

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::Fee;

/// 트랜잭션 타입
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionType {
    Deposit,
    Withdrawal,
}

/// 트랜잭션 상태
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Ok,
    Canceled,
    Failed,
}

/// 입출금 트랜잭션
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// 트랜잭션 ID
    pub id: String,
    /// 타임스탬프 (밀리초)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// ISO 8601 datetime
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    /// 업데이트 타임스탬프
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<i64>,
    /// 트랜잭션 타입
    #[serde(rename = "type")]
    pub tx_type: TransactionType,
    /// 화폐 코드
    pub currency: String,
    /// 네트워크
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// 금액
    pub amount: Decimal,
    /// 상태
    pub status: TransactionStatus,
    /// 주소
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// 태그 (memo, destination tag 등)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// 트랜잭션 해시
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// 수수료
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Fee>,
    /// 내부 전송 여부
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
    /// 컨펌 수
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u32>,
    /// 원본 응답
    #[serde(default)]
    pub info: serde_json::Value,
}

impl Transaction {
    /// 새 입금 트랜잭션 생성
    pub fn deposit(id: String, currency: String, amount: Decimal) -> Self {
        Self {
            id,
            timestamp: None,
            datetime: None,
            updated: None,
            tx_type: TransactionType::Deposit,
            currency,
            network: None,
            amount,
            status: TransactionStatus::Pending,
            address: None,
            tag: None,
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
            info: serde_json::Value::Null,
        }
    }

    /// 새 출금 트랜잭션 생성
    pub fn withdrawal(id: String, currency: String, amount: Decimal) -> Self {
        Self {
            id,
            timestamp: None,
            datetime: None,
            updated: None,
            tx_type: TransactionType::Withdrawal,
            currency,
            network: None,
            amount,
            status: TransactionStatus::Pending,
            address: None,
            tag: None,
            txid: None,
            fee: None,
            internal: None,
            confirmations: None,
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

    /// 주소 설정
    pub fn with_address(mut self, address: String, tag: Option<String>) -> Self {
        self.address = Some(address);
        self.tag = tag;
        self
    }

    /// 상태 설정
    pub fn with_status(mut self, status: TransactionStatus) -> Self {
        self.status = status;
        self
    }

    /// 트랜잭션 해시 설정
    pub fn with_txid(mut self, txid: String) -> Self {
        self.txid = Some(txid);
        self
    }

    /// 입금인지 확인
    pub fn is_deposit(&self) -> bool {
        self.tx_type == TransactionType::Deposit
    }

    /// 출금인지 확인
    pub fn is_withdrawal(&self) -> bool {
        self.tx_type == TransactionType::Withdrawal
    }

    /// 완료됨인지 확인
    pub fn is_completed(&self) -> bool {
        self.status == TransactionStatus::Ok
    }

    /// 대기 중인지 확인
    pub fn is_pending(&self) -> bool {
        self.status == TransactionStatus::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_deposit() {
        let tx = Transaction::deposit("123".into(), "BTC".into(), dec!(1.0))
            .with_timestamp(1700000000000)
            .with_status(TransactionStatus::Ok);

        assert!(tx.is_deposit());
        assert!(tx.is_completed());
    }

    #[test]
    fn test_withdrawal() {
        let tx = Transaction::withdrawal("456".into(), "ETH".into(), dec!(10.0))
            .with_address("0x1234...".into(), None)
            .with_txid("0xabcd...".into());

        assert!(tx.is_withdrawal());
        assert!(tx.is_pending());
        assert_eq!(tx.address, Some("0x1234...".into()));
    }
}
