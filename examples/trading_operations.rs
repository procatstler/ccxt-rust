//! Trading Operations Example
//!
//! Demonstrates trading operations (requires API keys):
//! - Fetching balances
//! - Creating orders
//! - Canceling orders
//! - Fetching open orders
//!
//! NOTE: This example requires valid API keys and will NOT execute
//! real trades unless you uncomment the trading sections.

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::cex::Binance;
use ccxt_rust::types::Exchange;
use ccxt_rust::CcxtResult;
use rust_decimal_macros::dec;
use std::env;

#[tokio::main]
async fn main() -> CcxtResult<()> {
    println!("=== Trading Operations Example ===\n");

    // Get API keys from environment variables
    let api_key = env::var("BINANCE_API_KEY").unwrap_or_else(|_| "".to_string());
    let secret = env::var("BINANCE_SECRET").unwrap_or_else(|_| "".to_string());

    if api_key.is_empty() || secret.is_empty() {
        println!("WARNING: API keys not set. Set BINANCE_API_KEY and BINANCE_SECRET");
        println!("         environment variables to run trading operations.\n");
        println!("Example:");
        println!("  export BINANCE_API_KEY='your_api_key'");
        println!("  export BINANCE_SECRET='your_secret'\n");

        // Still demonstrate public endpoints
        demonstrate_public_endpoints().await?;
        return Ok(());
    }

    // Create authenticated exchange instance
    let config = ExchangeConfig::new()
        .with_api_key(&api_key)
        .with_api_secret(&secret);

    let exchange = Binance::new(config)?;

    // Fetch account balances
    println!("--- Account Balances ---");
    let balances = exchange.fetch_balance().await?;

    println!("Non-zero balances:");
    for (currency, balance) in &balances.currencies {
        let free = balance.free.unwrap_or_default();
        let used = balance.used.unwrap_or_default();
        let total = balance.total.unwrap_or_default();
        if free > dec!(0) || used > dec!(0) {
            println!(
                "  {}: Free={}, Used={}, Total={}",
                currency, free, used, total
            );
        }
    }

    // Fetch open orders
    println!("\n--- Open Orders ---");
    let open_orders = exchange
        .fetch_open_orders(Some("BTC/USDT"), None, None)
        .await?;

    if open_orders.is_empty() {
        println!("No open orders for BTC/USDT");
    } else {
        for order in &open_orders {
            println!(
                "  Order {}: {:?} {} @ {} (Status: {:?})",
                order.id,
                order.side,
                order.amount,
                order.price.unwrap_or_default(),
                order.status
            );
        }
    }

    // Fetch order history
    println!("\n--- Recent Orders ---");
    let orders = exchange
        .fetch_orders(Some("BTC/USDT"), None, Some(5))
        .await?;

    for order in orders.iter().take(5) {
        println!(
            "  {} - {:?} {:?} {} @ {:?}",
            order.id, order.status, order.side, order.amount, order.price
        );
    }

    // Example: Create a limit order (COMMENTED OUT FOR SAFETY)
    /*
    println!("\n--- Creating Limit Order ---");
    let order = exchange.create_order(
        "BTC/USDT",
        OrderType::Limit,
        OrderSide::Buy,
        dec!(0.001),        // amount: 0.001 BTC
        Some(dec!(40000)),  // price: $40,000 (likely won't fill)
        None,
    ).await?;

    println!("Order created: {}", order.id);
    println!("  Symbol: {}", order.symbol);
    println!("  Type: {:?}", order.order_type);
    println!("  Side: {:?}", order.side);
    println!("  Amount: {}", order.amount);
    println!("  Price: {:?}", order.price);
    println!("  Status: {:?}", order.status);

    // Cancel the order
    println!("\n--- Canceling Order ---");
    let cancelled = exchange.cancel_order(&order.id, "BTC/USDT").await?;
    println!("Order {} cancelled: {:?}", cancelled.id, cancelled.status);
    */

    println!("\n=== Example Complete ===");
    Ok(())
}

async fn demonstrate_public_endpoints() -> CcxtResult<()> {
    println!("Demonstrating public endpoints only...\n");

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config)?;

    // Fetch ticker
    let ticker = exchange.fetch_ticker("BTC/USDT").await?;
    println!("BTC/USDT Ticker:");
    println!("  Last: {:?}", ticker.last);
    println!("  24h Change: {:?}%", ticker.percentage);

    // Fetch OHLCV
    println!("\nRecent OHLCV (1h candles):");
    let ohlcv = exchange
        .fetch_ohlcv(
            "BTC/USDT",
            ccxt_rust::types::Timeframe::Hour1,
            None,
            Some(5),
        )
        .await?;

    for candle in &ohlcv {
        println!(
            "  {} - O:{} H:{} L:{} C:{} V:{}",
            candle.timestamp, candle.open, candle.high, candle.low, candle.close, candle.volume
        );
    }

    Ok(())
}
