//! Keccak256 hashing utilities
//!
//! Ethereum에서 사용하는 Keccak256 해시 함수를 제공합니다.

#![allow(dead_code)]

use sha3::{Digest, Keccak256};

/// 데이터의 Keccak256 해시를 계산합니다.
///
/// # Arguments
///
/// * `data` - 해시할 데이터
///
/// # Returns
///
/// 32바이트 해시 값
///
/// # Example
///
/// ```rust
/// use ccxt_rust::crypto::evm::keccak256;
///
/// let hash = keccak256(b"hello");
/// assert_eq!(hash.len(), 32);
/// ```
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// 데이터의 Keccak256 해시를 hex 문자열로 반환합니다.
///
/// # Arguments
///
/// * `data` - 해시할 데이터
///
/// # Returns
///
/// 0x 접두사가 포함된 hex 문자열
///
/// # Example
///
/// ```rust
/// use ccxt_rust::crypto::evm::keccak256_hash;
///
/// let hash = keccak256_hash(b"hello");
/// assert!(hash.starts_with("0x"));
/// ```
pub fn keccak256_hash(data: &[u8]) -> String {
    format!("0x{}", hex::encode(keccak256(data)))
}

/// 여러 데이터 조각을 연결하여 Keccak256 해시를 계산합니다.
///
/// # Arguments
///
/// * `parts` - 연결하여 해시할 데이터 조각들
///
/// # Returns
///
/// 32바이트 해시 값
pub fn keccak256_concat(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// ABI 인코딩된 값들의 Keccak256 해시 (solidity keccak256(abi.encode(...)))
///
/// # Arguments
///
/// * `values` - ABI 인코딩된 값들 (각각 32바이트로 패딩됨)
///
/// # Returns
///
/// 32바이트 해시 값
pub fn keccak256_abi_encode(values: &[&[u8; 32]]) -> [u8; 32] {
    let mut data = Vec::with_capacity(values.len() * 32);
    for value in values {
        data.extend_from_slice(*value);
    }
    keccak256(&data)
}

/// 정수를 32바이트 big-endian 형식으로 패딩
pub fn pad_u256(value: u64) -> [u8; 32] {
    let mut result = [0u8; 32];
    result[24..].copy_from_slice(&value.to_be_bytes());
    result
}

/// 정수를 32바이트 big-endian 형식으로 패딩 (i64)
pub fn pad_i256(value: i64) -> [u8; 32] {
    let mut result = if value < 0 { [0xff; 32] } else { [0u8; 32] };
    result[24..].copy_from_slice(&value.to_be_bytes());
    result
}

/// 주소를 32바이트로 패딩 (왼쪽 12바이트 0으로 채움)
pub fn pad_address(address: &[u8; 20]) -> [u8; 32] {
    let mut result = [0u8; 32];
    result[12..].copy_from_slice(address);
    result
}

/// Bool을 32바이트로 패딩
pub fn pad_bool(value: bool) -> [u8; 32] {
    let mut result = [0u8; 32];
    if value {
        result[31] = 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keccak256_empty() {
        // keccak256("") = c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        let hash = keccak256(b"");
        let expected = hex::decode("c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").unwrap();
        assert_eq!(&hash[..], &expected[..]);
    }

    #[test]
    fn test_keccak256_hello() {
        // keccak256("hello") = 1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8
        let hash = keccak256(b"hello");
        let expected = hex::decode("1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8").unwrap();
        assert_eq!(&hash[..], &expected[..]);
    }

    #[test]
    fn test_keccak256_hash_format() {
        let hash = keccak256_hash(b"hello");
        assert!(hash.starts_with("0x"));
        assert_eq!(hash.len(), 66); // "0x" + 64 hex chars
    }

    #[test]
    fn test_keccak256_concat() {
        let a = b"hello";
        let b = b"world";

        let hash1 = keccak256_concat(&[a, b]);
        let hash2 = keccak256(b"helloworld");

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_pad_u256() {
        let padded = pad_u256(1);
        assert_eq!(padded[31], 1);
        assert_eq!(padded[..31], [0u8; 31]);
    }

    #[test]
    fn test_pad_address() {
        let address = [0xab; 20];
        let padded = pad_address(&address);
        assert_eq!(&padded[..12], &[0u8; 12]);
        assert_eq!(&padded[12..], &address[..]);
    }
}
