//! Cosmos Bech32 Address Encoding
//!
//! Cosmos SDK 체인을 위한 Bech32 주소 인코딩을 제공합니다.
//!
//! # 주소 형식
//!
//! Cosmos 주소는 다음과 같이 생성됩니다:
//! 1. 공개키 (33 bytes, 압축)
//! 2. SHA-256 해시
//! 3. RIPEMD-160 해시 (20 bytes)

#![allow(dead_code)]
//! 4. Bech32 인코딩 (prefix + data)
//!
//! # 예시
//!
//! - dYdX: dydx1abc...xyz
//! - Cosmos: cosmos1abc...xyz
//! - Osmosis: osmo1abc...xyz

use crate::errors::{CcxtError, CcxtResult};
use bech32::{Bech32, Hrp};
use ripemd::Ripemd160;
use sha2::{Sha256, Digest};
use super::keys::{private_key_to_public_key, coin_type};

/// 체인 설정
#[derive(Debug, Clone)]
pub struct ChainConfig {
    /// 체인 이름
    pub name: &'static str,
    /// Bech32 주소 접두사
    pub address_prefix: &'static str,
    /// SLIP-44 코인 타입
    pub coin_type: u32,
    /// 체인 ID
    pub chain_id: &'static str,
}

impl ChainConfig {
    /// 새 체인 설정 생성
    pub const fn new(
        name: &'static str,
        address_prefix: &'static str,
        coin_type: u32,
        chain_id: &'static str,
    ) -> Self {
        Self {
            name,
            address_prefix,
            coin_type,
            chain_id,
        }
    }
}

/// dYdX v4 메인넷
pub const DYDX_MAINNET: ChainConfig = ChainConfig::new(
    "dYdX v4 Mainnet",
    "dydx",
    coin_type::COSMOS,
    "dydx-mainnet-1",
);

/// dYdX v4 테스트넷
pub const DYDX_TESTNET: ChainConfig = ChainConfig::new(
    "dYdX v4 Testnet",
    "dydx",
    coin_type::COSMOS,
    "dydx-testnet-4",
);

/// Cosmos Hub
pub const COSMOS_HUB: ChainConfig = ChainConfig::new(
    "Cosmos Hub",
    "cosmos",
    coin_type::COSMOS,
    "cosmoshub-4",
);

/// Osmosis
pub const OSMOSIS: ChainConfig = ChainConfig::new(
    "Osmosis",
    "osmo",
    coin_type::COSMOS,
    "osmosis-1",
);

/// Injective (EVM 호환)
pub const INJECTIVE: ChainConfig = ChainConfig::new(
    "Injective",
    "inj",
    coin_type::ETHEREUM, // Injective는 EVM coin_type 사용
    "injective-1",
);

/// 공개키에서 Cosmos 주소 생성
///
/// # Process
/// 1. SHA-256(public_key)
/// 2. RIPEMD-160(sha256_result)
/// 3. Bech32 인코딩
///
/// # Arguments
///
/// * `public_key` - 33바이트 압축 공개키
/// * `prefix` - Bech32 주소 접두사 (예: "dydx", "cosmos")
///
/// # Returns
///
/// Bech32 인코딩된 주소 문자열
pub fn public_key_to_address(public_key: &[u8; 33], prefix: &str) -> CcxtResult<String> {
    // Step 1: SHA-256
    let sha256_hash = Sha256::digest(public_key);

    // Step 2: RIPEMD-160
    let ripemd_hash = Ripemd160::digest(sha256_hash);

    // Step 3: Bech32 인코딩
    let hrp = Hrp::parse(prefix)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid address prefix '{prefix}': {e}"),
        })?;

    let address = bech32::encode::<Bech32>(hrp, ripemd_hash.as_slice())
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Bech32 encoding failed: {e}"),
        })?;

    Ok(address)
}

/// 개인키에서 Cosmos 주소 생성 (편의 함수)
///
/// # Arguments
///
/// * `private_key` - 32바이트 개인키
/// * `prefix` - Bech32 주소 접두사
///
/// # Returns
///
/// Bech32 인코딩된 주소 문자열
pub fn private_key_to_address(private_key: &[u8; 32], prefix: &str) -> CcxtResult<String> {
    let public_key = private_key_to_public_key(private_key)?;
    public_key_to_address(&public_key, prefix)
}

/// Bech32 주소 검증
///
/// # Arguments
///
/// * `address` - Bech32 주소 문자열
/// * `expected_prefix` - 기대되는 접두사 (None이면 모든 접두사 허용)
///
/// # Returns
///
/// 유효하면 (prefix, data) 튜플
pub fn validate_address(address: &str, expected_prefix: Option<&str>) -> CcxtResult<(String, Vec<u8>)> {
    let (hrp, data) = bech32::decode(address)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid Bech32 address: {e}"),
        })?;

    let prefix = hrp.to_string();

    if let Some(expected) = expected_prefix {
        if prefix != expected {
            return Err(CcxtError::InvalidSignature {
                message: format!("Expected prefix '{expected}', got '{prefix}'"),
            });
        }
    }

    // 주소 데이터는 20바이트여야 함
    if data.len() != 20 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Expected 20 bytes address data, got {}", data.len()),
        });
    }

    Ok((prefix, data))
}

/// 주소 접두사 변환
///
/// 같은 공개키에서 다른 체인의 주소를 생성합니다.
///
/// # Arguments
///
/// * `address` - 원본 Bech32 주소
/// * `new_prefix` - 새 접두사
///
/// # Returns
///
/// 새 접두사로 인코딩된 주소
pub fn convert_address_prefix(address: &str, new_prefix: &str) -> CcxtResult<String> {
    let (_, data) = validate_address(address, None)?;

    let hrp = Hrp::parse(new_prefix)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid prefix '{new_prefix}': {e}"),
        })?;

    let new_address = bech32::encode::<Bech32>(hrp, &data)
        .map_err(|e| CcxtError::InvalidSignature {
            message: format!("Bech32 encoding failed: {e}"),
        })?;

    Ok(new_address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::cosmos::keys::derive_private_key;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_public_key_to_address() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let address = public_key_to_address(&keypair.public_key, "cosmos").unwrap();

        // cosmos로 시작해야 함
        assert!(address.starts_with("cosmos1"));

        // 주소 길이 확인 (cosmos1 + 38 chars)
        assert!(address.len() > 40);
    }

    #[test]
    fn test_dydx_address_generation() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let address = public_key_to_address(&keypair.public_key, "dydx").unwrap();

        assert!(address.starts_with("dydx1"));
    }

    #[test]
    fn test_private_key_to_address() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();

        let addr1 = public_key_to_address(&keypair.public_key, "cosmos").unwrap();
        let addr2 = private_key_to_address(&keypair.private_key, "cosmos").unwrap();

        assert_eq!(addr1, addr2);
    }

    #[test]
    fn test_validate_address() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let address = public_key_to_address(&keypair.public_key, "cosmos").unwrap();

        // 유효한 주소
        let result = validate_address(&address, Some("cosmos"));
        assert!(result.is_ok());

        let (prefix, data) = result.unwrap();
        assert_eq!(prefix, "cosmos");
        assert_eq!(data.len(), 20);

        // 잘못된 접두사 기대
        let result = validate_address(&address, Some("dydx"));
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_address_prefix() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let cosmos_addr = public_key_to_address(&keypair.public_key, "cosmos").unwrap();
        let dydx_addr = public_key_to_address(&keypair.public_key, "dydx").unwrap();

        // 접두사 변환
        let converted = convert_address_prefix(&cosmos_addr, "dydx").unwrap();
        assert_eq!(converted, dydx_addr);

        // 역변환
        let back = convert_address_prefix(&converted, "cosmos").unwrap();
        assert_eq!(back, cosmos_addr);
    }

    #[test]
    fn test_chain_configs() {
        assert_eq!(DYDX_MAINNET.address_prefix, "dydx");
        assert_eq!(DYDX_MAINNET.coin_type, 118);

        assert_eq!(COSMOS_HUB.address_prefix, "cosmos");
        assert_eq!(OSMOSIS.address_prefix, "osmo");

        // Injective는 EVM coin_type 사용
        assert_eq!(INJECTIVE.coin_type, 60);
    }

    #[test]
    fn test_deterministic_address() {
        // 동일한 입력은 항상 동일한 주소를 생성
        let keypair1 = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let keypair2 = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();

        let addr1 = public_key_to_address(&keypair1.public_key, "dydx").unwrap();
        let addr2 = public_key_to_address(&keypair2.public_key, "dydx").unwrap();

        assert_eq!(addr1, addr2);
    }
}
