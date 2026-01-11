//! secp256k1 ECDSA signing utilities
//!
//! Ethereum ECDSA 서명 및 주소 파생을 위한 유틸리티를 제공합니다.

#![allow(dead_code)]

use super::keccak::keccak256;
use crate::crypto::common::Signature;
use crate::errors::{CcxtError, CcxtResult};

use k256::{
    ecdsa::{RecoveryId, Signature as K256Signature, SigningKey, VerifyingKey},
    SecretKey,
};

/// 개인키에서 서명 키 생성
pub fn signing_key_from_bytes(private_key: &[u8]) -> CcxtResult<SigningKey> {
    let secret_key =
        SecretKey::from_slice(private_key).map_err(|e| CcxtError::InvalidSignature {
            message: format!("Invalid private key: {e}"),
        })?;
    Ok(SigningKey::from(secret_key))
}

/// 32바이트 해시에 서명합니다.
///
/// # Arguments
///
/// * `signing_key` - secp256k1 서명 키
/// * `hash` - 서명할 32바이트 해시
///
/// # Returns
///
/// ECDSA 서명 (r, s, v)
pub fn sign_hash(signing_key: &SigningKey, hash: &[u8; 32]) -> CcxtResult<Signature> {
    let (sig, recovery_id) =
        signing_key
            .sign_prehash_recoverable(hash)
            .map_err(|e| CcxtError::InvalidSignature {
                message: format!("Signing failed: {e}"),
            })?;

    let sig_bytes = sig.to_bytes();
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&sig_bytes[..32]);
    s.copy_from_slice(&sig_bytes[32..]);

    // Ethereum에서는 v = recovery_id + 27
    let v = recovery_id.to_byte() + 27;

    Ok(Signature::new(r, s, v))
}

/// 서명에서 공개키를 복구합니다.
///
/// # Arguments
///
/// * `hash` - 서명된 메시지의 해시
/// * `signature` - ECDSA 서명
///
/// # Returns
///
/// 복구된 공개키의 주소
pub fn recover_address(hash: &[u8; 32], signature: &Signature) -> CcxtResult<String> {
    // v 값에서 recovery_id 추출 (27 또는 28 -> 0 또는 1)
    // RecoveryId::new(is_y_odd, is_x_reduced)
    // - is_y_odd: recovery_id의 하위 비트 (0 = false, 1 = true)
    // - is_x_reduced: x 좌표가 축소되었는지 (일반적으로 false)
    let recovery_id = match signature.v {
        27 => RecoveryId::new(false, false), // recovery_id = 0
        28 => RecoveryId::new(true, false),  // recovery_id = 1
        v if v >= 35 => {
            // EIP-155: v = chain_id * 2 + 35 + recovery_id
            let is_y_odd = ((v - 35) % 2) == 1;
            RecoveryId::new(is_y_odd, false)
        },
        _ => {
            return Err(CcxtError::InvalidSignature {
                message: format!("Invalid v value: {}", signature.v),
            })
        },
    };

    // 서명 복원
    let mut sig_bytes = [0u8; 64];
    sig_bytes[..32].copy_from_slice(&signature.r);
    sig_bytes[32..].copy_from_slice(&signature.s);

    let sig = K256Signature::from_slice(&sig_bytes).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid signature: {e}"),
    })?;

    // 공개키 복구
    let verifying_key =
        VerifyingKey::recover_from_prehash(hash, &sig, recovery_id).map_err(|e| {
            CcxtError::InvalidSignature {
                message: format!("Recovery failed: {e}"),
            }
        })?;

    // 공개키에서 주소 계산
    Ok(verifying_key_to_address(&verifying_key))
}

/// 공개키에서 Ethereum 주소 계산
pub fn verifying_key_to_address(key: &VerifyingKey) -> String {
    let public_key = key.to_encoded_point(false);
    let public_key_bytes = public_key.as_bytes();

    // 첫 바이트(0x04)를 제외한 64바이트의 Keccak256 해시
    let hash = keccak256(&public_key_bytes[1..]);

    // 마지막 20바이트가 주소
    format!("0x{}", hex::encode(&hash[12..]))
}

/// 개인키에서 Ethereum 주소 계산
///
/// # Arguments
///
/// * `private_key` - 32바이트 개인키
///
/// # Returns
///
/// 체크섬 형식의 Ethereum 주소
pub fn private_key_to_address(private_key: &[u8]) -> CcxtResult<String> {
    let signing_key = signing_key_from_bytes(private_key)?;
    let verifying_key = signing_key.verifying_key();
    let address = verifying_key_to_address(verifying_key);
    Ok(to_checksum_address(&address))
}

/// EIP-55 체크섬 주소로 변환
///
/// # Arguments
///
/// * `address` - 0x로 시작하는 Ethereum 주소
///
/// # Returns
///
/// 체크섬이 적용된 주소
pub fn to_checksum_address(address: &str) -> String {
    let address_lower = address.to_lowercase();
    let address_hex = address_lower.strip_prefix("0x").unwrap_or(&address_lower);

    let hash = keccak256(address_hex.as_bytes());
    let hash_hex = hex::encode(hash);

    let mut result = String::with_capacity(42);
    result.push_str("0x");

    for (i, c) in address_hex.chars().enumerate() {
        if c.is_ascii_alphabetic() {
            let hash_char = hash_hex.chars().nth(i).unwrap();
            let hash_value = hash_char.to_digit(16).unwrap();
            if hash_value >= 8 {
                result.push(c.to_ascii_uppercase());
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// 주소 유효성 검사 (체크섬 포함)
pub fn is_valid_address(address: &str) -> bool {
    let address = match address.strip_prefix("0x") {
        Some(a) => a,
        None => return false,
    };

    if address.len() != 40 {
        return false;
    }

    // 모두 소문자 또는 대문자인 경우 체크섬 검사 불필요
    if address
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return true;
    }
    if address
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return true;
    }

    // 체크섬 검사
    let checksummed = to_checksum_address(&format!("0x{}", address.to_lowercase()));
    format!("0x{address}") == checksummed
}

/// Hex 문자열에서 개인키 파싱
pub fn parse_private_key(hex_str: &str) -> CcxtResult<[u8; 32]> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);

    let bytes = hex::decode(hex_str).map_err(|e| CcxtError::InvalidSignature {
        message: format!("Invalid hex: {e}"),
    })?;

    if bytes.len() != 32 {
        return Err(CcxtError::InvalidSignature {
            message: format!("Private key must be 32 bytes, got {}", bytes.len()),
        });
    }

    let mut result = [0u8; 32];
    result.copy_from_slice(&bytes);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDRESS: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    #[test]
    fn test_private_key_to_address() {
        let private_key = parse_private_key(TEST_PRIVATE_KEY).unwrap();
        let address = private_key_to_address(&private_key).unwrap();
        assert_eq!(address.to_lowercase(), TEST_ADDRESS.to_lowercase());
    }

    #[test]
    fn test_checksum_address() {
        let address = "0xfb6916095ca1df60bb79ce92ce3ea74c37c5d359";
        let checksummed = to_checksum_address(address);
        assert_eq!(checksummed, "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359");
    }

    #[test]
    fn test_sign_and_recover() {
        let private_key = parse_private_key(TEST_PRIVATE_KEY).unwrap();
        let signing_key = signing_key_from_bytes(&private_key).unwrap();

        let message = b"Hello, Ethereum!";
        let hash = keccak256(message);

        let signature = sign_hash(&signing_key, &hash).unwrap();
        let recovered = recover_address(&hash, &signature).unwrap();

        let expected_address = private_key_to_address(&private_key).unwrap();
        assert_eq!(recovered.to_lowercase(), expected_address.to_lowercase());
    }

    #[test]
    fn test_valid_address() {
        assert!(is_valid_address(
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        ));
        assert!(is_valid_address(
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        ));
        assert!(!is_valid_address(
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb9226"
        )); // 짧음
        assert!(!is_valid_address(
            "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        )); // 0x 없음
    }

    #[test]
    fn test_parse_private_key() {
        let key = parse_private_key(TEST_PRIVATE_KEY).unwrap();
        assert_eq!(key.len(), 32);

        let key_no_prefix = parse_private_key(&TEST_PRIVATE_KEY[2..]).unwrap();
        assert_eq!(key, key_no_prefix);
    }
}
