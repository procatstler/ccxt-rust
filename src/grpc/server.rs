//! ccxt-rust gRPC Server
//!
//! Standalone gRPC server for exchange operations.
//!
//! Usage:
//!   cargo run --bin ccxt-grpc-server --features "grpc,cex"
//!
//! Environment variables:
//!   GRPC_PORT: Server port (default: 50051)
//!   RUST_LOG: Log level (default: info)

use std::net::SocketAddr;
use tonic::transport::Server;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

// Include the library
use ccxt_rust::grpc::generated::exchange_service_server::ExchangeServiceServer;
use ccxt_rust::grpc::ExchangeServiceImpl;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    // Get port from environment or use default
    let port: u16 = std::env::var("GRPC_PORT")
        .unwrap_or_else(|_| "50051".to_string())
        .parse()
        .expect("Invalid GRPC_PORT");

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;

    info!("Starting ccxt-rust gRPC server on {}", addr);

    // Create service
    let exchange_service = ExchangeServiceImpl::new();

    // Build and start server
    Server::builder()
        .add_service(ExchangeServiceServer::new(exchange_service))
        .serve(addr)
        .await?;

    Ok(())
}
