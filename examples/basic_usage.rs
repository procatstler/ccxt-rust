//! Basic Usage Example
//!
//! Demonstrates fundamental operations with ccxt-rust:
//! - Creating exchange instances
//! - Fetching market data
//! - Working with tickers and order books

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::Exchange;
use ccxt_rust::CcxtResult;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== CCXT-Rust Basic Usage Example ===\n");

    // Create exchange instance (no API keys needed for public endpoints)
    let config = ExchangeConfig::new();
    let exchange = Binance::new(config)?;

    // Display exchange info
    println!("Exchange: {}", exchange.name());
    println!("Exchange ID: {:?}", exchange.id());

    // Check exchange capabilities
    let features = exchange.has();
    println!("\nExchange Features:");
    println!("  - Spot Trading: {}", features.spot);
    println!("  - Margin Trading: {}", features.margin);
    println!("  - Futures Trading: {}", features.swap);
    println!("  - WebSocket: {}", features.ws);

    // Fetch markets
    println!("\n--- Fetching Markets ---");
    let markets = exchange.fetch_markets().await?;
    println!("Total markets available: {}", markets.len());

    // Show first 5 markets
    println!("\nSample markets:");
    for market in markets.iter().take(5) {
        println!(
            "  {} - Base: {}, Quote: {}",
            market.symbol, market.base, market.quote
        );
    }

    // Fetch ticker for BTC/USDT
    println!("\n--- Fetching Ticker ---");
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;
    println!("BTC/USDT Ticker:");
    println!("  Last Price: {:?}", ticker.last);
    println!("  Bid: {:?}", ticker.bid);
    println!("  Ask: {:?}", ticker.ask);
    println!("  24h High: {:?}", ticker.high);
    println!("  24h Low: {:?}", ticker.low);
    println!("  24h Volume: {:?}", ticker.base_volume);

    // Fetch order book
    println!("\n--- Fetching Order Book ---");
    let orderbook = exchange.fetch_order_book("BTC/USDT", Some(5)).await?;
    println!("BTC/USDT Order Book (Top 5):");
    println!("  Bids:");
    for bid in orderbook.bids.iter().take(5) {
        println!("    Price: {}, Amount: {}", bid.price, bid.amount);
    }
    println!("  Asks:");
    for ask in orderbook.asks.iter().take(5) {
        println!("    Price: {}, Amount: {}", ask.price, ask.amount);
    }

    // Fetch recent trades
    println!("\n--- Fetching Recent Trades ---");
    let trades = exchange.fetch_trades("BTC/USDT", None, Some(5)).await?;
    println!("Recent BTC/USDT Trades:");
    for trade in trades.iter().take(5) {
        println!(
            "  ID: {}, Price: {}, Amount: {}, Side: {:?}",
            trade.id, trade.price, trade.amount, trade.side
        );
    }

    println!("\n=== Example Complete ===");
    Ok(())
}
