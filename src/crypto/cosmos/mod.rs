//! Cosmos SDK Cryptography Module
//!
//! Cosmos SDK 기반 체인을 위한 암호화 유틸리티를 제공합니다.
//!
//! # 주요 기능
//!
//! - BIP-32/44 HD 키 파생
//! - Bech32 주소 인코딩
//! - secp256k1 ECDSA 서명
//! - Cosmos 트랜잭션 서명
//!
//! # 지원 체인
//!
//! - dYdX v4 (주소 접두사: "dydx")
//! - Cosmos Hub (주소 접두사: "cosmos")
//! - Osmosis (주소 접두사: "osmo")
//! - 기타 Cosmos SDK 기반 체인
//!
//! # 사용 예시
//!
//! ```rust,ignore
//! use ccxt_rust::crypto::cosmos::{CosmosWallet, ChainConfig};
//!
//! // dYdX v4 지갑 생성
//! let config = ChainConfig::dydx_mainnet();
//! let wallet = CosmosWallet::from_mnemonic(&mnemonic, &config, 0)?;
//!
//! // 주소 확인
//! println!("Address: {}", wallet.address());
//!
//! // 메시지 서명
//! let signature = wallet.sign_bytes(&message)?;
//! ```

mod keys;
mod address;
mod signer;
mod wallet;

pub use keys::{
    derive_private_key, derive_private_key_from_seed,
    mnemonic_to_seed, private_key_to_public_key,
    CosmosKeyPair,
};
pub use address::{
    public_key_to_address, private_key_to_address,
    ChainConfig, DYDX_MAINNET, DYDX_TESTNET,
    COSMOS_HUB, OSMOSIS, INJECTIVE,
};
pub use signer::{
    sign_bytes, sign_amino, verify_signature,
    CosmosSignature,
};
pub use wallet::CosmosWallet;
