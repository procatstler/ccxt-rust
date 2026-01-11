//! DEX Trading Example
//!
//! Demonstrates decentralized exchange operations:
//! - Hyperliquid perpetual futures
//! - dYdX v4 trading
//!
//! NOTE: DEX trading requires wallet setup and proper credentials.

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::dex::Hyperliquid;
use ccxt_rust::types::Exchange;
use ccxt_rust::CcxtResult;
use std::env;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== DEX Trading Example ===\n");

    // Example 1: Hyperliquid (public endpoints)
    hyperliquid_example().await?;

    println!("\n=== Example Complete ===");
    Ok(())
}

async fn hyperliquid_example() -> CcxtResult<()> {
    println!("--- Hyperliquid ---");

    // Check for private key (optional)
    let private_key = env::var("HYPERLIQUID_PRIVATE_KEY").ok();

    let config = ExchangeConfig::new();
    let exchange = if let Some(pk) = private_key {
        println!("Using authenticated mode");
        Hyperliquid::from_private_key(config, &pk)?
    } else {
        println!("Using public mode (no trading available)");
        Hyperliquid::new(config)?
    };

    // Fetch markets
    println!("\nFetching markets...");
    let markets = exchange.fetch_markets().await?;
    println!("Available markets: {}", markets.len());

    // Show perpetual markets
    println!("\nPerpetual markets (first 10):");
    for market in markets.iter().take(10) {
        println!("  {} - Type: {:?}", market.symbol, market.market_type);
    }

    // Fetch ticker for BTC
    println!("\nFetching BTC ticker...");
    let ticker = exchange.fetch_ticker("BTC/USDC:USDC").await?;
    println!("BTC Perpetual:");
    println!("  Last: {:?}", ticker.last);
    println!("  Bid: {:?}", ticker.bid);
    println!("  Ask: {:?}", ticker.ask);
    println!("  Mark Price: {:?}", ticker.mark_price);
    println!("  Index Price: {:?}", ticker.index_price);

    // Fetch order book
    println!("\nFetching order book...");
    let orderbook = exchange.fetch_order_book("BTC/USDC:USDC", Some(5)).await?;
    println!("Top 5 Bids:");
    for bid in orderbook.bids.iter().take(5) {
        println!("  {} @ {}", bid.amount, bid.price);
    }
    println!("Top 5 Asks:");
    for ask in orderbook.asks.iter().take(5) {
        println!("  {} @ {}", ask.amount, ask.price);
    }

    // Fetch funding rate
    println!("\nFetching funding rate...");
    match exchange.fetch_funding_rate("BTC/USDC:USDC").await {
        Ok(funding) => {
            println!("BTC Funding Rate:");
            println!("  Current Rate: {:?}", funding.funding_rate);
            println!("  Next Funding: {:?}", funding.funding_datetime);
        },
        Err(e) => println!("Could not fetch funding rate: {}", e),
    }

    Ok(())
}
