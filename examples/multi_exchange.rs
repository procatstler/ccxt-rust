//! Multi-Exchange Example
//!
//! Demonstrates how to work with multiple exchanges simultaneously
//! for price comparison and arbitrage detection.

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::cex::{Binance, Bybit, Okx};
use ccxt_rust::types::Exchange;
use ccxt_rust::CcxtResult;
use rust_decimal::Decimal;
use std::time::Instant;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== Multi-Exchange Price Comparison ===\n");

    let config = ExchangeConfig::new();

    // Create multiple exchange instances
    let binance = Binance::new(config.clone())?;
    let bybit = Bybit::new(config.clone())?;
    let okx = Okx::new(config)?;

    let symbol = "BTC/USDT";

    // Fetch tickers from all exchanges in parallel
    println!("Fetching {} prices from multiple exchanges...\n", symbol);
    let start = Instant::now();

    let (binance_ticker, bybit_ticker, okx_ticker) = tokio::join!(
        binance.fetch_ticker(symbol),
        bybit.fetch_ticker(symbol),
        okx.fetch_ticker(symbol),
    );

    let elapsed = start.elapsed();
    println!("Fetched all prices in {:?}\n", elapsed);

    // Collect results
    let mut prices: Vec<(&str, Option<Decimal>)> = Vec::new();

    if let Ok(ticker) = binance_ticker {
        println!(
            "Binance  - Last: {:?}, Bid: {:?}, Ask: {:?}",
            ticker.last, ticker.bid, ticker.ask
        );
        prices.push(("Binance", ticker.last));
    }

    if let Ok(ticker) = bybit_ticker {
        println!(
            "Bybit    - Last: {:?}, Bid: {:?}, Ask: {:?}",
            ticker.last, ticker.bid, ticker.ask
        );
        prices.push(("Bybit", ticker.last));
    }

    if let Ok(ticker) = okx_ticker {
        println!(
            "OKX      - Last: {:?}, Bid: {:?}, Ask: {:?}",
            ticker.last, ticker.bid, ticker.ask
        );
        prices.push(("OKX", ticker.last));
    }

    // Calculate price differences
    println!("\n--- Price Analysis ---");

    let valid_prices: Vec<(&str, Decimal)> = prices
        .into_iter()
        .filter_map(|(name, price)| price.map(|p| (name, p)))
        .collect();

    if valid_prices.len() >= 2 {
        let min = valid_prices.iter().min_by_key(|(_, p)| p).unwrap();
        let max = valid_prices.iter().max_by_key(|(_, p)| p).unwrap();

        let spread = max.1 - min.1;
        let spread_percent = (spread / min.1) * Decimal::from(100);

        println!("Lowest Price:  {} at {}", min.1, min.0);
        println!("Highest Price: {} at {}", max.1, max.0);
        println!("Spread: {} ({:.4}%)", spread, spread_percent);

        if spread_percent > Decimal::from(1) {
            println!("\n⚠️  Potential arbitrage opportunity detected!");
            println!("    Buy on {} -> Sell on {}", min.0, max.0);
        }
    }

    println!("\n=== Example Complete ===");
    Ok(())
}
