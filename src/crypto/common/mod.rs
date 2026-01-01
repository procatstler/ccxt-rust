//! Common cryptographic traits and utilities
//!
//! DEX 서명을 위한 공통 인터페이스를 정의합니다.

mod traits;

pub use traits::{Signature, Signer, TypedDataHasher};
