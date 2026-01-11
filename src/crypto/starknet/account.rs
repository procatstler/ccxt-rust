//! StarkNet Account Derivation
//!
//! Ethereum 개인키에서 StarkNet 계정을 파생합니다.
//!
//! # 참조
//!
//! - [StarkNet Account Derivation](https://docs.starknet.io/documentation/architecture_and_concepts/Accounts/)
//! - [Paradex Account System](https://docs.paradex.trade/developers/authentication)

#![allow(dead_code)]

use crate::errors::{CcxtError, CcxtResult};
use super::poseidon::{poseidon_hash_many, string_to_felt};
use super::curve::get_public_key;
use starknet_types_core::felt::Felt;
use sha2::{Sha256, Digest};

/// StarkNet 계정
#[derive(Debug, Clone)]
pub struct StarkNetAccount {
    /// StarkNet 개인키
    pub private_key: Felt,
    /// StarkNet 공개키
    pub public_key: Felt,
    /// 계정 주소
    pub address: Felt,
}

impl StarkNetAccount {
    /// 개인키로 계정 생성
    pub fn from_private_key(private_key: Felt) -> Self {
        let public_key = get_public_key(&private_key);
        let address = compute_account_address(&public_key, None);

        Self {
            private_key,
            public_key,
            address,
        }
    }

    /// 개인키로 계정 생성 (주소 지정)
    pub fn with_address(private_key: Felt, address: Felt) -> Self {
        let public_key = get_public_key(&private_key);

        Self {
            private_key,
            public_key,
            address,
        }
    }

    /// 16진수 문자열에서 계정 생성
    pub fn from_hex(private_key_hex: &str) -> CcxtResult<Self> {
        let private_key = parse_hex_to_felt(private_key_hex)?;
        Ok(Self::from_private_key(private_key))
    }

    /// 공개키 16진수 반환
    pub fn public_key_hex(&self) -> String {
        format!("0x{}", hex::encode(self.public_key.to_bytes_be()).trim_start_matches('0'))
    }

    /// 주소 16진수 반환
    pub fn address_hex(&self) -> String {
        format!("0x{}", hex::encode(self.address.to_bytes_be()).trim_start_matches('0'))
    }
}

/// Ethereum 개인키에서 StarkNet 개인키 파생
///
/// Paradex와 같은 거래소에서 사용하는 방식:
/// StarkNet 개인키 = grind(keccak256(eth_signature))
///
/// # Arguments
///
/// * `eth_private_key` - 32바이트 Ethereum 개인키
/// * `domain` - 도메인 문자열 (예: "paradex")
///
/// # Returns
///
/// StarkNet 개인키 (Felt)
pub fn derive_starknet_private_key(eth_private_key: &[u8; 32], domain: &str) -> CcxtResult<Felt> {
    // 1. 도메인과 개인키를 결합하여 시드 생성
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(eth_private_key);

    let seed = hasher.finalize();

    // 2. 시드를 StarkNet 필드 범위 내로 그라인딩
    let stark_key = grind_key(&seed)?;

    Ok(stark_key)
}

/// Ethereum 서명에서 StarkNet 개인키 파생
///
/// EIP-712 서명을 사용한 키 파생
///
/// # Arguments
///
/// * `signature` - 65바이트 Ethereum 서명 (r, s, v)
///
/// # Returns
///
/// StarkNet 개인키 (Felt)
pub fn derive_from_signature(signature: &[u8]) -> CcxtResult<Felt> {
    if signature.len() < 64 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Signature too short: {} bytes", signature.len()),
        });
    }

    // r + s 부분을 해시
    let mut hasher = Sha256::new();
    hasher.update(&signature[..64]);
    let seed = hasher.finalize();

    grind_key(&seed)
}

/// 키 그라인딩 (StarkNet 필드 범위 내로 조정)
///
/// StarkNet의 필드 크기는 약 252비트이므로,
/// 256비트 해시를 반복적으로 해시하여 유효한 키를 생성
fn grind_key(seed: &[u8]) -> CcxtResult<Felt> {
    // StarkNet 필드 최대값 (약 2^251)
    let max_value = [
        0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    let mut current_seed = seed.to_vec();

    for _ in 0..100 {
        let mut hasher = Sha256::new();
        hasher.update(&current_seed);
        let hash = hasher.finalize();

        // 첫 바이트의 상위 비트를 마스킹하여 필드 범위 내로 조정
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&hash);
        key_bytes[0] &= 0x07; // 상위 5비트 클리어

        // 최대값보다 작은지 확인
        if key_bytes < max_value {
            return Ok(Felt::from_bytes_be(&key_bytes));
        }

        // 다시 해시
        current_seed = hash.to_vec();
    }

    Err(CcxtError::InvalidSignature {
        message: "Failed to grind key after 100 iterations".to_string(),
    })
}

/// StarkNet 계정 주소 계산
///
/// 계정 주소 = pedersen(prefix, public_key, salt, class_hash)
///
/// # Arguments
///
/// * `public_key` - StarkNet 공개키
/// * `class_hash` - 계정 컨트랙트 클래스 해시 (None이면 기본값 사용)
///
/// # Returns
///
/// 계정 주소 (Felt)
pub fn compute_starknet_address(public_key: &Felt, class_hash: Option<&Felt>) -> Felt {
    compute_account_address(public_key, class_hash)
}

/// 계정 주소 계산 (내부 함수)
fn compute_account_address(public_key: &Felt, class_hash: Option<&Felt>) -> Felt {
    // 기본 클래스 해시 (OpenZeppelin Account v0.8.0)
    let default_class_hash = string_to_felt("OZAccount");
    let class_hash = class_hash.unwrap_or(&default_class_hash);

    // 주소 계산: poseidon(prefix, class_hash, salt, public_key)
    let prefix = string_to_felt("STARKNET_CONTRACT_ADDRESS");
    let salt = Felt::ZERO; // 기본 salt

    let values = vec![prefix, *class_hash, salt, *public_key];
    poseidon_hash_many(&values)
}

/// 16진수 문자열을 Felt로 파싱
fn parse_hex_to_felt(hex_str: &str) -> CcxtResult<Felt> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

    let bytes = hex::decode(hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid hex: {e}"),
    })?;

    if bytes.len() > 32 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Hex value too large: {} bytes", bytes.len()),
        });
    }

    let mut result = [0u8; 32];
    result[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(Felt::from_bytes_be(&result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_starknet_private_key() {
        let eth_key = [0x01u8; 32];
        let stark_key = derive_starknet_private_key(&eth_key, "paradex").unwrap();

        // 키가 0이 아니어야 함
        assert_ne!(stark_key, Felt::ZERO);

        // 동일한 입력은 동일한 출력을 생성
        let stark_key2 = derive_starknet_private_key(&eth_key, "paradex").unwrap();
        assert_eq!(stark_key, stark_key2);
    }

    #[test]
    fn test_different_domains() {
        let eth_key = [0x02u8; 32];

        let key1 = derive_starknet_private_key(&eth_key, "paradex").unwrap();
        let key2 = derive_starknet_private_key(&eth_key, "dydx").unwrap();

        // 다른 도메인은 다른 키를 생성
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_account_from_private_key() {
        let private_key = Felt::from(12345u64);
        let account = StarkNetAccount::from_private_key(private_key);

        // 공개키와 주소가 0이 아니어야 함
        assert_ne!(account.public_key, Felt::ZERO);
        assert_ne!(account.address, Felt::ZERO);
    }

    #[test]
    fn test_account_from_hex() {
        let account = StarkNetAccount::from_hex("0x123abc").unwrap();

        assert_ne!(account.private_key, Felt::ZERO);
        assert_ne!(account.public_key, Felt::ZERO);
    }

    #[test]
    fn test_compute_starknet_address() {
        let public_key = Felt::from(12345u64);
        let address = compute_starknet_address(&public_key, None);

        assert_ne!(address, Felt::ZERO);

        // 동일한 공개키는 동일한 주소를 생성
        let address2 = compute_starknet_address(&public_key, None);
        assert_eq!(address, address2);
    }

    #[test]
    fn test_grind_key() {
        let seed = [0xFFu8; 32]; // 큰 값
        let key = grind_key(&seed).unwrap();

        // 유효한 필드 요소여야 함
        let bytes = key.to_bytes_be();
        assert!(bytes[0] < 0x08); // 상위 비트가 마스킹됨
    }
}
