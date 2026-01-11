//! BIP-32/44 HD Key Derivation for Cosmos SDK
//!
//! Cosmos SDK 체인을 위한 계층적 결정적(HD) 키 파생을 제공합니다.
//!
//! # BIP-44 경로 규격
//!
//! - Cosmos 표준: m/44'/118'/0'/0/{index}
//! - dYdX v4: m/44'/118'/0'/0/{index} (coin_type = 118)
//! - Injective: m/44'/60'/0'/0/{index} (EVM 호환)
//!
//! # 참조
//!
//! - [BIP-32](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki)
//! - [BIP-44](https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki)
//! - [SLIP-44](https://github.com/satoshilabs/slips/blob/master/slip-0044.md)

use crate::errors::{CcxtError, CcxtResult};
use bip32::{DerivationPath, XPrv};
use bip39::Mnemonic;
use k256::ecdsa::SigningKey;

/// SLIP-44 Coin Types
pub mod coin_type {
    /// Cosmos Hub (ATOM)
    pub const COSMOS: u32 = 118;
    /// Ethereum (for EVM-compatible chains like Injective)
    pub const ETHEREUM: u32 = 60;
}

/// Cosmos 키 쌍
#[derive(Clone)]
pub struct CosmosKeyPair {
    /// 개인키 (32 bytes)
    pub private_key: [u8; 32],
    /// 압축 공개키 (33 bytes)
    pub public_key: [u8; 33],
}

impl CosmosKeyPair {
    /// 개인키에서 키 쌍 생성
    pub fn from_private_key(private_key: [u8; 32]) -> CcxtResult<Self> {
        let public_key = private_key_to_public_key(&private_key)?;
        Ok(Self {
            private_key,
            public_key,
        })
    }

    /// 개인키 hex 문자열 반환
    pub fn private_key_hex(&self) -> String {
        hex::encode(self.private_key)
    }

    /// 공개키 hex 문자열 반환
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key)
    }
}

impl std::fmt::Debug for CosmosKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CosmosKeyPair")
            .field("public_key", &self.public_key_hex())
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

/// 니모닉에서 시드 생성
///
/// BIP-39 니모닉 문구를 시드 바이트로 변환합니다.
///
/// # Arguments
///
/// * `mnemonic` - BIP-39 니모닉 문구 (12/24 단어)
/// * `passphrase` - 선택적 패스프레이즈 (기본: 빈 문자열)
///
/// # Returns
///
/// 64바이트 시드
pub fn mnemonic_to_seed(mnemonic_str: &str, passphrase: &str) -> CcxtResult<[u8; 64]> {
    let mnemonic =
        Mnemonic::parse_normalized(mnemonic_str).map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid mnemonic: {e}"),
        })?;

    let seed = mnemonic.to_seed(passphrase);
    let mut result = [0u8; 64];
    result.copy_from_slice(&seed);
    Ok(result)
}

/// 시드에서 개인키 파생
///
/// BIP-44 경로를 사용하여 시드에서 개인키를 파생합니다.
///
/// # Arguments
///
/// * `seed` - 64바이트 시드
/// * `coin_type` - SLIP-44 코인 타입 (118 = Cosmos, 60 = Ethereum)
/// * `account` - 계정 인덱스 (일반적으로 0)
/// * `index` - 주소 인덱스
///
/// # Returns
///
/// 32바이트 개인키
pub fn derive_private_key_from_seed(
    seed: &[u8; 64],
    coin_type: u32,
    account: u32,
    index: u32,
) -> CcxtResult<[u8; 32]> {
    // BIP-44 경로: m/44'/{coin_type}'/{account}'/0/{index}
    let path_str = format!("m/44'/{coin_type}'/{account}'/0/{index}");
    let path: DerivationPath = path_str.parse().map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid derivation path: {e}"),
    })?;

    let xprv = XPrv::derive_from_path(seed, &path).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Key derivation failed: {e}"),
    })?;

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(&xprv.private_key().to_bytes());
    Ok(private_key)
}

/// 니모닉에서 개인키 파생 (편의 함수)
///
/// # Arguments
///
/// * `mnemonic` - BIP-39 니모닉 문구
/// * `coin_type` - SLIP-44 코인 타입
/// * `index` - 주소 인덱스
///
/// # Returns
///
/// CosmosKeyPair (개인키 + 공개키)
pub fn derive_private_key(mnemonic: &str, coin_type: u32, index: u32) -> CcxtResult<CosmosKeyPair> {
    let seed = mnemonic_to_seed(mnemonic, "")?;
    let private_key = derive_private_key_from_seed(&seed, coin_type, 0, index)?;
    CosmosKeyPair::from_private_key(private_key)
}

/// 개인키에서 압축 공개키 생성
///
/// secp256k1 곡선을 사용하여 개인키에서 압축 공개키를 계산합니다.
///
/// # Arguments
///
/// * `private_key` - 32바이트 개인키
///
/// # Returns
///
/// 33바이트 압축 공개키
pub fn private_key_to_public_key(private_key: &[u8; 32]) -> CcxtResult<[u8; 33]> {
    let signing_key =
        SigningKey::from_bytes(private_key.into()).map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid private key: {e}"),
        })?;

    let verifying_key = signing_key.verifying_key();
    let compressed = verifying_key.to_encoded_point(true);

    let mut result = [0u8; 33];
    result.copy_from_slice(compressed.as_bytes());
    Ok(result)
}

/// Hex 문자열에서 개인키 파싱
pub fn parse_private_key(hex_str: &str) -> CcxtResult<[u8; 32]> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid hex: {e}"),
    })?;

    if bytes.len() != 32 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Expected 32 bytes, got {}", bytes.len()),
        });
    }

    let mut result = [0u8; 32];
    result.copy_from_slice(&bytes);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 테스트용 니모닉 (절대 실제 자금 사용 금지!)
    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_mnemonic_to_seed() {
        let seed = mnemonic_to_seed(TEST_MNEMONIC, "").unwrap();
        assert_eq!(seed.len(), 64);

        // 동일한 니모닉은 동일한 시드를 생성
        let seed2 = mnemonic_to_seed(TEST_MNEMONIC, "").unwrap();
        assert_eq!(seed, seed2);

        // 패스프레이즈가 다르면 다른 시드
        let seed3 = mnemonic_to_seed(TEST_MNEMONIC, "password").unwrap();
        assert_ne!(seed, seed3);
    }

    #[test]
    fn test_derive_private_key() {
        let keypair = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();

        // 개인키는 32바이트
        assert_eq!(keypair.private_key.len(), 32);

        // 공개키는 33바이트 (압축)
        assert_eq!(keypair.public_key.len(), 33);

        // 압축 공개키는 02 또는 03으로 시작
        assert!(keypair.public_key[0] == 0x02 || keypair.public_key[0] == 0x03);
    }

    #[test]
    fn test_different_indices_produce_different_keys() {
        let keypair0 = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let keypair1 = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 1).unwrap();

        assert_ne!(keypair0.private_key, keypair1.private_key);
        assert_ne!(keypair0.public_key, keypair1.public_key);
    }

    #[test]
    fn test_different_coin_types_produce_different_keys() {
        let cosmos_key = derive_private_key(TEST_MNEMONIC, coin_type::COSMOS, 0).unwrap();
        let eth_key = derive_private_key(TEST_MNEMONIC, coin_type::ETHEREUM, 0).unwrap();

        assert_ne!(cosmos_key.private_key, eth_key.private_key);
    }

    #[test]
    fn test_private_key_to_public_key() {
        // 알려진 테스트 벡터
        let private_key = [1u8; 32]; // 단순 테스트용
        let public_key = private_key_to_public_key(&private_key).unwrap();

        assert_eq!(public_key.len(), 33);
        assert!(public_key[0] == 0x02 || public_key[0] == 0x03);
    }

    #[test]
    fn test_keypair_from_private_key() {
        let private_key = [42u8; 32];
        let keypair = CosmosKeyPair::from_private_key(private_key).unwrap();

        assert_eq!(keypair.private_key, private_key);
        assert_eq!(keypair.public_key.len(), 33);
    }

    #[test]
    fn test_parse_private_key() {
        let hex_str = "0x0101010101010101010101010101010101010101010101010101010101010101";
        let private_key = parse_private_key(hex_str).unwrap();
        assert_eq!(private_key, [1u8; 32]);

        // 0x 없이도 동작
        let hex_str2 = "0101010101010101010101010101010101010101010101010101010101010101";
        let private_key2 = parse_private_key(hex_str2).unwrap();
        assert_eq!(private_key2, [1u8; 32]);
    }
}
