//! Bullish WebSocket Example
//!
//! Demonstrates how to use the Bullish WebSocket client to subscribe to real-time data.
//!
//! Run with: cargo run --example bullish_ws_example

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::foreign::BullishWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Bullish WebSocket Example\n");

    // Create Bullish WebSocket client
    let config = ExchangeConfig::default();
    let client = BullishWs::new(config);

    // Example 1: Watch ticker
    println!("Example 1: Watching BTC/USDC ticker...");
    let mut ticker_rx = client.watch_ticker("BTC/USDC").await?;

    // Receive ticker updates for a few seconds
    let ticker_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = ticker_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Ticker stream connected"),
                WsMessage::Ticker(event) => {
                    println!(
                        "Ticker: {} - Last: {:?}, Bid: {:?}, Ask: {:?}, Change: {:?}%",
                        event.symbol,
                        event.ticker.last,
                        event.ticker.bid,
                        event.ticker.ask,
                        event.ticker.percentage
                    );
                    count += 1;
                    if count >= 3 {
                        break;
                    }
                }
                WsMessage::Error(err) => eprintln!("Error: {}", err),
                _ => {}
            }
        }
    });

    // Wait for ticker task
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), ticker_task).await;

    // Example 2: Watch order book
    println!("\nExample 2: Watching BTC/USDC order book...");
    let client = BullishWs::new(ExchangeConfig::default());
    let mut book_rx = client.watch_order_book("BTC/USDC", None).await?;

    let book_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = book_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Order book stream connected"),
                WsMessage::OrderBook(event) => {
                    println!(
                        "Order Book: {} - {} bids, {} asks (snapshot: {})",
                        event.symbol,
                        event.order_book.bids.len(),
                        event.order_book.asks.len(),
                        event.is_snapshot
                    );
                    if !event.order_book.bids.is_empty() && !event.order_book.asks.is_empty() {
                        println!(
                            "  Best bid: {} @ {}, Best ask: {} @ {}",
                            event.order_book.bids[0].amount,
                            event.order_book.bids[0].price,
                            event.order_book.asks[0].amount,
                            event.order_book.asks[0].price
                        );
                    }
                    count += 1;
                    if count >= 3 {
                        break;
                    }
                }
                WsMessage::Error(err) => eprintln!("Error: {}", err),
                _ => {}
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), book_task).await;

    // Example 3: Watch trades
    println!("\nExample 3: Watching BTC/USDC trades...");
    let client = BullishWs::new(ExchangeConfig::default());
    let mut trades_rx = client.watch_trades("BTC/USDC").await?;

    let trades_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = trades_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Trades stream connected"),
                WsMessage::Trade(event) => {
                    for trade in &event.trades {
                        println!(
                            "Trade: {} - {} @ {} ({:?})",
                            event.symbol,
                            trade.amount,
                            trade.price,
                            trade.side
                        );
                        count += 1;
                        if count >= 5 {
                            break;
                        }
                    }
                    if count >= 5 {
                        break;
                    }
                }
                WsMessage::Error(err) => eprintln!("Error: {}", err),
                _ => {}
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(30), trades_task).await;

    println!("\nExample completed!");

    Ok(())
}
