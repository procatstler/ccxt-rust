//! StarkNet Wallet
//!
//! StarkNet 서명 및 트랜잭션 생성을 위한 지갑 구현
//!
//! # 참조
//!
//! - [StarkNet Accounts](https://docs.starknet.io/documentation/architecture_and_concepts/Accounts/)
//! - [Paradex Authentication](https://docs.paradex.trade/developers/authentication)

use super::account::{derive_starknet_private_key, StarkNetAccount};
use super::curve::{sign_hash, StarkNetSignature};
use super::typed_data::{StarkNetDomain, StarkNetTypedData, StarkNetTypedDataField};
use crate::errors::CcxtResult;
use starknet_types_core::felt::Felt;
use std::collections::HashMap;

/// StarkNet 지갑
#[derive(Debug, Clone)]
pub struct StarkNetWallet {
    /// 계정 정보
    account: StarkNetAccount,
    /// 도메인 (거래소 식별용)
    domain: String,
}

impl StarkNetWallet {
    /// StarkNet 개인키로 지갑 생성
    pub fn new(private_key: Felt, domain: impl Into<String>) -> Self {
        let account = StarkNetAccount::from_private_key(private_key);
        Self {
            account,
            domain: domain.into(),
        }
    }

    /// 16진수 StarkNet 개인키로 지갑 생성
    pub fn from_hex(private_key_hex: &str, domain: impl Into<String>) -> CcxtResult<Self> {
        let account = StarkNetAccount::from_hex(private_key_hex)?;
        Ok(Self {
            account,
            domain: domain.into(),
        })
    }

    /// Ethereum 개인키에서 StarkNet 지갑 파생
    ///
    /// Paradex와 같은 거래소에서 사용
    pub fn from_eth_private_key(
        eth_private_key: &[u8; 32],
        domain: impl Into<String>,
    ) -> CcxtResult<Self> {
        let domain_str: String = domain.into();
        let stark_key = derive_starknet_private_key(eth_private_key, &domain_str)?;
        let account = StarkNetAccount::from_private_key(stark_key);

        Ok(Self {
            account,
            domain: domain_str,
        })
    }

    /// 주소와 함께 지갑 생성 (외부에서 주소를 알고 있는 경우)
    pub fn with_address(private_key: Felt, address: Felt, domain: impl Into<String>) -> Self {
        let account = StarkNetAccount::with_address(private_key, address);
        Self {
            account,
            domain: domain.into(),
        }
    }

    /// 공개키 반환
    pub fn public_key(&self) -> &Felt {
        &self.account.public_key
    }

    /// 주소 반환
    pub fn address(&self) -> &Felt {
        &self.account.address
    }

    /// 공개키 16진수 반환
    pub fn public_key_hex(&self) -> String {
        self.account.public_key_hex()
    }

    /// 주소 16진수 반환
    pub fn address_hex(&self) -> String {
        self.account.address_hex()
    }

    /// 메시지 해시 서명
    pub fn sign(&self, message_hash: &Felt) -> CcxtResult<StarkNetSignature> {
        sign_hash(&self.account.private_key, message_hash, None)
    }

    /// 타입 데이터 서명 (SNIP-12)
    pub fn sign_typed_data(&self, typed_data: &StarkNetTypedData) -> CcxtResult<StarkNetSignature> {
        let hash = typed_data.sign_hash(&self.account.address)?;
        self.sign(&hash)
    }

    /// 주문 서명 (Paradex 형식)
    ///
    /// # Arguments
    ///
    /// * `order` - 주문 데이터
    /// * `chain_id` - StarkNet 체인 ID
    ///
    /// # Returns
    ///
    /// (r, s) 서명 튜플
    pub fn sign_order(&self, order: &ParadexOrder, chain_id: &str) -> CcxtResult<(String, String)> {
        let typed_data = order.to_typed_data(&self.domain, chain_id)?;
        let signature = self.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }

    /// 취소 요청 서명
    pub fn sign_cancel(
        &self,
        order_ids: &[String],
        chain_id: &str,
    ) -> CcxtResult<(String, String)> {
        let typed_data = create_cancel_typed_data(&self.domain, order_ids, chain_id)?;
        let signature = self.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }

    /// 인증 메시지 서명
    pub fn sign_auth_message(
        &self,
        message: &str,
        timestamp: u64,
        chain_id: &str,
    ) -> CcxtResult<(String, String)> {
        let typed_data = create_auth_typed_data(&self.domain, message, timestamp, chain_id)?;
        let signature = self.sign_typed_data(&typed_data)?;
        Ok(signature.to_hex())
    }
}

/// Paradex 주문 구조
#[derive(Debug, Clone)]
pub struct ParadexOrder {
    /// 마켓 심볼
    pub market: String,
    /// 주문 방향 (Buy/Sell)
    pub side: String,
    /// 주문 타입 (Limit/Market)
    pub order_type: String,
    /// 수량
    pub size: String,
    /// 가격 (Limit 주문시)
    pub price: Option<String>,
    /// 클라이언트 주문 ID
    pub client_id: Option<String>,
    /// 타임스탬프
    pub timestamp: u64,
    /// 만료 시간
    pub expiry: Option<u64>,
}

impl ParadexOrder {
    /// 새 주문 생성
    pub fn new(
        market: impl Into<String>,
        side: impl Into<String>,
        order_type: impl Into<String>,
        size: impl Into<String>,
    ) -> Self {
        Self {
            market: market.into(),
            side: side.into(),
            order_type: order_type.into(),
            size: size.into(),
            price: None,
            client_id: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            expiry: None,
        }
    }

    /// 가격 설정
    pub fn with_price(mut self, price: impl Into<String>) -> Self {
        self.price = Some(price.into());
        self
    }

    /// 클라이언트 ID 설정
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    /// 만료 시간 설정
    pub fn with_expiry(mut self, expiry: u64) -> Self {
        self.expiry = Some(expiry);
        self
    }

    /// SNIP-12 타입 데이터로 변환
    fn to_typed_data(&self, domain_name: &str, chain_id: &str) -> CcxtResult<StarkNetTypedData> {
        let domain = StarkNetDomain::new(domain_name, "1").with_chain_id(chain_id);

        let mut types = HashMap::new();
        types.insert(
            "Order".to_string(),
            vec![
                StarkNetTypedDataField::new("market", "string"),
                StarkNetTypedDataField::new("side", "string"),
                StarkNetTypedDataField::new("orderType", "string"),
                StarkNetTypedDataField::new("size", "string"),
                StarkNetTypedDataField::new("price", "string"),
                StarkNetTypedDataField::new("timestamp", "u64"),
            ],
        );

        let message = serde_json::json!({
            "market": self.market,
            "side": self.side,
            "orderType": self.order_type,
            "size": self.size,
            "price": self.price.clone().unwrap_or_default(),
            "timestamp": self.timestamp,
        });

        Ok(StarkNetTypedData::new(domain, "Order", types, message))
    }
}

/// 취소 요청 타입 데이터 생성
fn create_cancel_typed_data(
    domain_name: &str,
    order_ids: &[String],
    chain_id: &str,
) -> CcxtResult<StarkNetTypedData> {
    let domain = StarkNetDomain::new(domain_name, "1").with_chain_id(chain_id);

    let mut types = HashMap::new();
    types.insert(
        "Cancel".to_string(),
        vec![
            StarkNetTypedDataField::new("orderIds", "string"),
            StarkNetTypedDataField::new("timestamp", "u64"),
        ],
    );

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let message = serde_json::json!({
        "orderIds": order_ids.join(","),
        "timestamp": timestamp,
    });

    Ok(StarkNetTypedData::new(domain, "Cancel", types, message))
}

/// 인증 타입 데이터 생성
fn create_auth_typed_data(
    domain_name: &str,
    message: &str,
    timestamp: u64,
    chain_id: &str,
) -> CcxtResult<StarkNetTypedData> {
    let domain = StarkNetDomain::new(domain_name, "1").with_chain_id(chain_id);

    let mut types = HashMap::new();
    types.insert(
        "Auth".to_string(),
        vec![
            StarkNetTypedDataField::new("message", "string"),
            StarkNetTypedDataField::new("timestamp", "u64"),
        ],
    );

    let msg = serde_json::json!({
        "message": message,
        "timestamp": timestamp,
    });

    Ok(StarkNetTypedData::new(domain, "Auth", types, msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_new() {
        let private_key = Felt::from(12345u64);
        let wallet = StarkNetWallet::new(private_key, "paradex");

        assert_ne!(*wallet.public_key(), Felt::ZERO);
        assert_ne!(*wallet.address(), Felt::ZERO);
    }

    #[test]
    fn test_wallet_from_hex() {
        let wallet = StarkNetWallet::from_hex("0x123abc", "paradex").unwrap();

        assert_ne!(*wallet.public_key(), Felt::ZERO);
        assert!(!wallet.public_key_hex().is_empty());
    }

    #[test]
    fn test_wallet_from_eth_key() {
        let eth_key = [0x01u8; 32];
        let wallet = StarkNetWallet::from_eth_private_key(&eth_key, "paradex").unwrap();

        assert_ne!(*wallet.public_key(), Felt::ZERO);
    }

    #[test]
    fn test_sign_message() {
        let private_key = Felt::from(12345u64);
        let wallet = StarkNetWallet::new(private_key, "test");

        let message_hash = Felt::from(67890u64);
        let signature = wallet.sign(&message_hash).unwrap();

        assert_ne!(signature.r, Felt::ZERO);
        assert_ne!(signature.s, Felt::ZERO);
    }

    #[test]
    fn test_paradex_order() {
        let order = ParadexOrder::new("ETH-USD", "Buy", "Limit", "1.0")
            .with_price("2000.0")
            .with_client_id("order123");

        assert_eq!(order.market, "ETH-USD");
        assert_eq!(order.side, "Buy");
        assert_eq!(order.price, Some("2000.0".to_string()));
    }

    #[test]
    fn test_sign_order() {
        let private_key = Felt::from(12345u64);
        let wallet = StarkNetWallet::new(private_key, "paradex");

        let order = ParadexOrder::new("ETH-USD", "Buy", "Limit", "1.0").with_price("2000.0");

        let (r, s) = wallet.sign_order(&order, "SN_MAIN").unwrap();

        assert!(r.starts_with("0x"));
        assert!(s.starts_with("0x"));
    }

    #[test]
    fn test_sign_auth_message() {
        let private_key = Felt::from(12345u64);
        let wallet = StarkNetWallet::new(private_key, "paradex");

        let (r, s) = wallet
            .sign_auth_message("Login to Paradex", 1234567890, "SN_MAIN")
            .unwrap();

        assert!(r.starts_with("0x"));
        assert!(s.starts_with("0x"));
    }
}
