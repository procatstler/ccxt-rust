//! Cryptographic utilities for API signing

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 서명 생성
pub fn hmac_sha256(secret: &str, message: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// HMAC-SHA256 서명을 hex 문자열로 반환
pub fn hmac_sha256_hex(secret: &str, message: &str) -> String {
    hex::encode(hmac_sha256(secret, message))
}

/// Base64 인코딩
pub fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Base64 디코딩
pub fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data)
}
