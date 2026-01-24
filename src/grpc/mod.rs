//! gRPC Server Module
//!
//! Provides gRPC service for exchange operations, allowing external services
//! (like tracevault-collector) to use ccxt-rust functionality via gRPC.

#[cfg(feature = "grpc")]
pub mod generated {
    #![allow(clippy::all)]
    #![allow(warnings)]
    include!("generated/ccxt.exchange.v1.rs");
}

#[cfg(feature = "grpc")]
pub mod service;

#[cfg(feature = "grpc")]
pub use service::ExchangeServiceImpl;
