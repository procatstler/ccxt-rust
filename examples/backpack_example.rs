//! Backpack REST API Example
//!
//! Demonstrates using the Backpack exchange REST API
//! Run with: cargo run --example backpack_example

use ccxt_rust::exchanges::foreign::Backpack;
use ccxt_rust::types::Exchange;
use ccxt_rust::ExchangeConfig;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Backpack Exchange REST API Example");
    println!("===================================\n");

    // Create exchange instance with longer timeout
    let config = ExchangeConfig::default().with_timeout(60000);
    let exchange = Backpack::new(config)?;

    println!("Exchange: {} ({})", exchange.name(), exchange.id());
    println!();

    // Fetch markets
    println!("Fetching markets...");
    let markets = exchange.fetch_markets().await?;
    println!("Found {} markets", markets.len());

    // Show first few markets
    for market in markets.iter().take(5) {
        println!("  {} - {:?} ({})",
            market.symbol,
            market.market_type,
            if market.active { "active" } else { "inactive" }
        );
    }
    println!();

    // Fetch ticker for BTC/USDC
    println!("Fetching BTC/USDC ticker...");
    match exchange.fetch_ticker("BTC/USDC").await {
        Ok(ticker) => {
            println!("  Symbol: {}", ticker.symbol);
            println!("  Last: {:?}", ticker.last);
            println!("  Bid: {:?}", ticker.bid);
            println!("  Ask: {:?}", ticker.ask);
            println!("  24h High: {:?}", ticker.high);
            println!("  24h Low: {:?}", ticker.low);
            println!("  Volume: {:?}", ticker.base_volume);
        }
        Err(e) => println!("  Error: {}", e),
    }
    println!();

    // Fetch order book
    println!("Fetching BTC/USDC order book...");
    match exchange.fetch_order_book("BTC/USDC", Some(5)).await {
        Ok(orderbook) => {
            println!("  Top 5 Bids:");
            for bid in orderbook.bids.iter().take(5) {
                println!("    {} @ {}", bid.amount, bid.price);
            }
            println!("  Top 5 Asks:");
            for ask in orderbook.asks.iter().take(5) {
                println!("    {} @ {}", ask.amount, ask.price);
            }
        }
        Err(e) => println!("  Error: {}", e),
    }
    println!();

    // Fetch recent trades
    println!("Fetching recent BTC/USDC trades...");
    match exchange.fetch_trades("BTC/USDC", None, Some(5)).await {
        Ok(trades) => {
            for trade in trades.iter().take(5) {
                println!("  {} {} @ {} - {} {}",
                    trade.datetime.clone().unwrap_or_else(|| "?".to_string()),
                    trade.side.clone().unwrap_or_else(|| "?".to_string()),
                    trade.price,
                    trade.amount,
                    trade.symbol
                );
            }
        }
        Err(e) => println!("  Error: {}", e),
    }

    println!("\nDone!");
    Ok(())
}
