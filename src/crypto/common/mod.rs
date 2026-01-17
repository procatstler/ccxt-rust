//! Common cryptographic traits and utilities
//!
//! DEX 서명 및 API 인증을 위한 공통 인터페이스와 유틸리티를 정의합니다.
//!
//! ## 모듈 구성
//!
//! - [`traits`]: 서명 및 해싱 인터페이스 (Signer, TypedDataHasher)
//! - [`rsa`]: RSA 서명 유틸리티 (RSA-SHA256, RSA-SHA512) - native only
//! - [`totp`]: TOTP 2FA 유틸리티 (RFC 6238)
//! - [`jwt`]: JWT 인코딩/디코딩 유틸리티 - native only

mod traits;

// Native-only modules (require ring/jsonwebtoken)
#[cfg(feature = "native")]
pub mod jwt;
#[cfg(feature = "native")]
pub mod rsa;

// Works on all platforms
pub mod totp;

pub use traits::{Signature, Signer, TypedDataHasher};

// RSA 편의 함수 재export (native only)
#[cfg(feature = "native")]
pub use rsa::{rsa_sign_sha256, rsa_sign_sha256_base64, rsa_sign_sha512, rsa_sign_sha512_base64};

// TOTP 편의 함수 재export
pub use totp::{generate_totp, verify_totp};

// JWT 편의 함수 재export (native only)
#[cfg(feature = "native")]
pub use jwt::{
    decode_jwt_hs256, decode_jwt_rs256, encode_jwt_hs256, encode_jwt_rs256, parse_jwt_claims,
};
