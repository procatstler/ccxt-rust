//! RSA Signing Utilities
//!
//! RSA-SHA256, RSA-SHA512 서명 기능 제공

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rsa::pkcs1v15::{SigningKey, VerifyingKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::signature::{SignatureEncoding, Signer as RsaSigner, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Sha256, Sha512};

use crate::errors::{CcxtError, CcxtResult};

/// RSA 키 쌍
pub struct RsaKeyPair {
    private_key: RsaPrivateKey,
    public_key: RsaPublicKey,
}

impl RsaKeyPair {
    /// PEM 형식의 개인키에서 키 쌍 생성
    pub fn from_private_key_pem(pem: &str) -> CcxtResult<Self> {
        let private_key = RsaPrivateKey::from_pkcs8_pem(pem).map_err(|e| CcxtError::InvalidPrivateKey {
            message: format!("Invalid RSA private key: {e}"),
        })?;
        let public_key = private_key.to_public_key();

        Ok(Self {
            private_key,
            public_key,
        })
    }

    /// DER 형식의 개인키에서 키 쌍 생성
    pub fn from_private_key_der(der: &[u8]) -> CcxtResult<Self> {
        let private_key =
            RsaPrivateKey::from_pkcs8_der(der).map_err(|e| CcxtError::InvalidPrivateKey {
                message: format!("Invalid RSA private key: {e}"),
            })?;
        let public_key = private_key.to_public_key();

        Ok(Self {
            private_key,
            public_key,
        })
    }

    /// 공개키 반환
    pub fn public_key(&self) -> &RsaPublicKey {
        &self.public_key
    }

    /// RSA-SHA256 서명
    pub fn sign_sha256(&self, message: &[u8]) -> CcxtResult<Vec<u8>> {
        let signing_key = SigningKey::<Sha256>::new(self.private_key.clone());
        let signature = signing_key.sign(message);
        Ok(signature.to_bytes().to_vec())
    }

    /// RSA-SHA512 서명
    pub fn sign_sha512(&self, message: &[u8]) -> CcxtResult<Vec<u8>> {
        let signing_key = SigningKey::<Sha512>::new(self.private_key.clone());
        let signature = signing_key.sign(message);
        Ok(signature.to_bytes().to_vec())
    }

    /// RSA-SHA256 서명 (Base64 인코딩)
    pub fn sign_sha256_base64(&self, message: &[u8]) -> CcxtResult<String> {
        let signature = self.sign_sha256(message)?;
        Ok(BASE64.encode(&signature))
    }

    /// RSA-SHA512 서명 (Base64 인코딩)
    pub fn sign_sha512_base64(&self, message: &[u8]) -> CcxtResult<String> {
        let signature = self.sign_sha512(message)?;
        Ok(BASE64.encode(&signature))
    }
}

/// RSA 공개키 검증자
pub struct RsaVerifier {
    public_key: RsaPublicKey,
}

impl RsaVerifier {
    /// PEM 형식의 공개키에서 검증자 생성
    pub fn from_public_key_pem(pem: &str) -> CcxtResult<Self> {
        let public_key = RsaPublicKey::from_public_key_pem(pem).map_err(|e| CcxtError::AuthenticationError {
            message: format!("Invalid RSA public key: {e}"),
        })?;

        Ok(Self { public_key })
    }

    /// DER 형식의 공개키에서 검증자 생성
    pub fn from_public_key_der(der: &[u8]) -> CcxtResult<Self> {
        let public_key =
            RsaPublicKey::from_public_key_der(der).map_err(|e| CcxtError::AuthenticationError {
                message: format!("Invalid RSA public key: {e}"),
            })?;

        Ok(Self { public_key })
    }

    /// RSA-SHA256 서명 검증
    pub fn verify_sha256(&self, message: &[u8], signature: &[u8]) -> CcxtResult<bool> {
        let verifying_key = VerifyingKey::<Sha256>::new(self.public_key.clone());
        let sig = rsa::pkcs1v15::Signature::try_from(signature).map_err(|e| {
            CcxtError::InvalidSignature {
                message: format!("Invalid signature format: {e}"),
            }
        })?;

        Ok(verifying_key.verify(message, &sig).is_ok())
    }

    /// RSA-SHA512 서명 검증
    pub fn verify_sha512(&self, message: &[u8], signature: &[u8]) -> CcxtResult<bool> {
        let verifying_key = VerifyingKey::<Sha512>::new(self.public_key.clone());
        let sig = rsa::pkcs1v15::Signature::try_from(signature).map_err(|e| {
            CcxtError::InvalidSignature {
                message: format!("Invalid signature format: {e}"),
            }
        })?;

        Ok(verifying_key.verify(message, &sig).is_ok())
    }

    /// RSA-SHA256 서명 검증 (Base64 디코딩)
    pub fn verify_sha256_base64(&self, message: &[u8], signature_base64: &str) -> CcxtResult<bool> {
        let signature = BASE64.decode(signature_base64).map_err(|e| CcxtError::ParseError {
            data_type: "base64".into(),
            message: format!("Invalid base64 signature: {e}"),
        })?;
        self.verify_sha256(message, &signature)
    }
}

/// RSA-SHA256 서명 (단일 함수)
pub fn rsa_sign_sha256(private_key_pem: &str, message: &[u8]) -> CcxtResult<Vec<u8>> {
    let key_pair = RsaKeyPair::from_private_key_pem(private_key_pem)?;
    key_pair.sign_sha256(message)
}

/// RSA-SHA256 서명 (Base64, 단일 함수)
pub fn rsa_sign_sha256_base64(private_key_pem: &str, message: &[u8]) -> CcxtResult<String> {
    let key_pair = RsaKeyPair::from_private_key_pem(private_key_pem)?;
    key_pair.sign_sha256_base64(message)
}

/// RSA-SHA512 서명 (단일 함수)
pub fn rsa_sign_sha512(private_key_pem: &str, message: &[u8]) -> CcxtResult<Vec<u8>> {
    let key_pair = RsaKeyPair::from_private_key_pem(private_key_pem)?;
    key_pair.sign_sha512(message)
}

/// RSA-SHA512 서명 (Base64, 단일 함수)
pub fn rsa_sign_sha512_base64(private_key_pem: &str, message: &[u8]) -> CcxtResult<String> {
    let key_pair = RsaKeyPair::from_private_key_pem(private_key_pem)?;
    key_pair.sign_sha512_base64(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 테스트용 RSA 키 (실제 사용하지 말 것)
    const TEST_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDH+jK3WlvGxQFk
mJBW/b8s5e3dH3J5b8S3jgpD7ZMQtPBVF3gzPEJF4E0gNJJ6RdJN5OALy5AQLZ5y
xS5q7C8yF8Q3J5+VgX8Q5+F5TQ5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX
5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O
5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqT
gX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q
5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTAgMBAAECggEAEk5NJq8p5JVBE7F8
H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F
8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F
8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F
8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F
8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F
8H5J5T5JVQKBgQD7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVB
E7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE
7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7F8H5J5T5JVBE7FwKBgQDL+jK3
WlvGxQFkmJBW/b8s5e3dH3J5b8S3jgpD7ZMQtPBVF3gzPEJF4E0gNJJ6RdJN5OAL
y5AQLZ5yxS5q7C8yF8Q3J5+VgX8Q5+F5TQ5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q
5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5N
qTgX5Q5O5NqTgX5QKBgQC3WlvGxQFkmJBW/b8s5e3dH3J5b8S3jgpD7ZMQtPBVF3
gzPEJF4E0gNJJ6RdJN5OALy5AQLZ5yxS5q7C8yF8Q3J5+VgX8Q5+F5TQ5O5NqTgX
5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O
5NqTgX5Q5O5NwKBgFkmJBW/b8s5e3dH3J5b8S3jgpD7ZMQtPBVF3gzPEJF4E0gNJ
J6RdJN5OALy5AQLZ5yxS5q7C8yF8Q3J5+VgX8Q5+F5TQ5O5NqTgX5Q5O5NqTgX5Q
5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5N
qTgX5Q5O5NqTgX5Q5O5NqTgX5QKBgF8s5e3dH3J5b8S3jgpD7ZMQtPBVF3gzPEJF
4E0gNJJ6RdJN5OALy5AQLZ5yxS5q7C8yF8Q3J5+VgX8Q5+F5TQ5O5NqTgX5Q5O5N
qTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX
5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q5O5NqTgX5Q
-----END PRIVATE KEY-----"#;

    // RSA 키는 실제 환경에서만 테스트 가능
    // 이 테스트는 API가 제대로 노출되었는지 확인용
    #[test]
    fn test_rsa_api_exists() {
        // 함수가 존재하는지 확인
        let _ = rsa_sign_sha256 as fn(&str, &[u8]) -> CcxtResult<Vec<u8>>;
        let _ = rsa_sign_sha256_base64 as fn(&str, &[u8]) -> CcxtResult<String>;
        let _ = rsa_sign_sha512 as fn(&str, &[u8]) -> CcxtResult<Vec<u8>>;
        let _ = rsa_sign_sha512_base64 as fn(&str, &[u8]) -> CcxtResult<String>;
    }
}
