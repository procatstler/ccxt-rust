//! EVM Wallet Management
//!
//! EVM 호환 체인을 위한 지갑 관리 기능을 제공합니다.

#![allow(dead_code)]

use crate::errors::CcxtResult;
use crate::crypto::common::{Signature, Signer, TypedDataHasher};
use super::keccak::keccak256;
use super::secp256k1::{
    sign_hash, signing_key_from_bytes, private_key_to_address,
    parse_private_key,
};
use super::eip712::Eip712TypedData;
use async_trait::async_trait;
use k256::ecdsa::SigningKey;

/// EVM 지갑
///
/// Ethereum 및 EVM 호환 체인에서 메시지 서명을 위한 지갑입니다.
///
/// # Example
///
/// ```rust,ignore
/// use ccxt_rust::crypto::evm::EvmWallet;
///
/// let wallet = EvmWallet::from_private_key("0x...")?;
/// println!("Address: {}", wallet.address());
///
/// let signature = wallet.sign_message(b"Hello").await?;
/// ```
pub struct EvmWallet {
    /// 서명 키
    signing_key: SigningKey,
    /// 체크섬 형식의 주소
    address: String,
}

impl EvmWallet {
    /// 개인키에서 지갑 생성
    ///
    /// # Arguments
    ///
    /// * `private_key` - 32바이트 개인키 (0x 접두사 선택)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let wallet = EvmWallet::from_private_key(
    ///     "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    /// )?;
    /// ```
    pub fn from_private_key(private_key: &str) -> CcxtResult<Self> {
        let key_bytes = parse_private_key(private_key)?;
        let signing_key = signing_key_from_bytes(&key_bytes)?;
        let address = private_key_to_address(&key_bytes)?;

        Ok(Self {
            signing_key,
            address,
        })
    }

    /// 바이트 배열에서 지갑 생성
    pub fn from_bytes(private_key: &[u8; 32]) -> CcxtResult<Self> {
        let signing_key = signing_key_from_bytes(private_key)?;
        let address = private_key_to_address(private_key)?;

        Ok(Self {
            signing_key,
            address,
        })
    }

    /// EIP-712 타입 데이터 서명
    ///
    /// # Arguments
    ///
    /// * `typed_data` - EIP-712 형식의 타입 데이터
    ///
    /// # Returns
    ///
    /// ECDSA 서명 (r, s, v)
    pub async fn sign_typed_data(&self, typed_data: &Eip712TypedData) -> CcxtResult<Signature> {
        let hash = typed_data.sign_hash()?;
        self.sign_hash(&hash).await
    }

    /// personal_sign 스타일 메시지 서명 (동기)
    ///
    /// 메시지에 "\x19Ethereum Signed Message:\n{len}" 접두사 추가
    pub fn sign_message_sync(&self, message: &[u8]) -> CcxtResult<Signature> {
        let prefixed = create_personal_sign_message(message);
        let hash = keccak256(&prefixed);
        sign_hash(&self.signing_key, &hash)
    }

    /// 해시 직접 서명 (동기)
    pub fn sign_hash_sync(&self, hash: &[u8; 32]) -> CcxtResult<Signature> {
        sign_hash(&self.signing_key, hash)
    }

    /// 주소 반환 (소문자)
    pub fn address_lowercase(&self) -> String {
        self.address.to_lowercase()
    }
}

#[async_trait]
impl Signer for EvmWallet {
    /// 체크섬 형식의 주소 반환
    fn address(&self) -> &str {
        &self.address
    }

    /// personal_sign 스타일 메시지 서명
    ///
    /// 메시지에 "\x19Ethereum Signed Message:\n{len}" 접두사를 추가하고 서명
    async fn sign_message(&self, message: &[u8]) -> CcxtResult<Signature> {
        self.sign_message_sync(message)
    }

    /// 해시 직접 서명
    async fn sign_hash(&self, hash: &[u8; 32]) -> CcxtResult<Signature> {
        self.sign_hash_sync(hash)
    }
}

impl std::fmt::Debug for EvmWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmWallet")
            .field("address", &self.address)
            .finish()
    }
}

/// personal_sign 메시지 생성
///
/// "\x19Ethereum Signed Message:\n{len}{message}" 형식으로 변환
pub fn create_personal_sign_message(message: &[u8]) -> Vec<u8> {
    let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
    let mut result = prefix.into_bytes();
    result.extend_from_slice(message);
    result
}

/// personal_sign 해시 계산
pub fn personal_sign_hash(message: &[u8]) -> [u8; 32] {
    let prefixed = create_personal_sign_message(message);
    keccak256(&prefixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDRESS: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    #[test]
    fn test_wallet_from_private_key() {
        let wallet = EvmWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        assert_eq!(wallet.address().to_lowercase(), TEST_ADDRESS.to_lowercase());
    }

    #[test]
    fn test_wallet_from_bytes() {
        let key_bytes = parse_private_key(TEST_PRIVATE_KEY).unwrap();
        let wallet = EvmWallet::from_bytes(&key_bytes).unwrap();
        assert_eq!(wallet.address().to_lowercase(), TEST_ADDRESS.to_lowercase());
    }

    #[tokio::test]
    async fn test_sign_message() {
        let wallet = EvmWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        let message = b"Hello, Ethereum!";

        let signature = wallet.sign_message(message).await.unwrap();

        // 서명이 유효한 형식인지 확인
        assert_eq!(signature.r.len(), 32);
        assert_eq!(signature.s.len(), 32);
        assert!(signature.v == 27 || signature.v == 28);
    }

    #[tokio::test]
    async fn test_sign_hash() {
        let wallet = EvmWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        let hash = keccak256(b"test message");

        let signature = wallet.sign_hash(&hash).await.unwrap();

        // 서명이 유효한 형식인지 확인
        assert_eq!(signature.to_bytes().len(), 65);
    }

    #[test]
    fn test_personal_sign_message() {
        let message = b"Hello, World!";
        let prefixed = create_personal_sign_message(message);

        let expected_prefix = "\x19Ethereum Signed Message:\n13";
        assert!(prefixed.starts_with(expected_prefix.as_bytes()));
        assert!(prefixed.ends_with(message));
    }

    #[tokio::test]
    async fn test_sign_typed_data() {
        use crate::crypto::evm::{Eip712Domain, Eip712TypedData, TypedDataField};
        use std::collections::HashMap;

        let wallet = EvmWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();

        let domain = Eip712Domain::new("Test", "1", 1);

        let mut types = HashMap::new();
        types.insert(
            "Message".to_string(),
            vec![TypedDataField::new("content", "string")],
        );

        let message = serde_json::json!({
            "content": "Hello"
        });

        let typed_data = Eip712TypedData::new(domain, "Message", types, message);
        let signature = wallet.sign_typed_data(&typed_data).await.unwrap();

        assert_eq!(signature.to_bytes().len(), 65);
    }

    #[test]
    fn test_wallet_debug() {
        let wallet = EvmWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        let debug_str = format!("{wallet:?}");

        // 개인키가 노출되지 않는지 확인
        assert!(debug_str.contains("address"));
        assert!(!debug_str.contains("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"));
    }
}
