//! Exmo WebSocket Example
//!
//! Demonstrates how to use the Exmo WebSocket client to subscribe to real-time data.
//!
//! Run with: cargo run --example exmo_ws_example

use ccxt_rust::client::ExchangeConfig;
use ccxt_rust::exchanges::ExmoWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Exmo WebSocket Example\n");

    // Create Exmo WebSocket client
    let config = ExchangeConfig::default();
    let client = ExmoWs::new(config);

    // Example 1: Watch ticker
    println!("Example 1: Watching BTC/USD ticker...");
    let mut ticker_rx = client.watch_ticker("BTC/USD").await?;

    // Receive ticker updates for a few seconds
    let ticker_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = ticker_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Ticker stream connected"),
                WsMessage::Ticker(event) => {
                    println!(
                        "Ticker: {} - Last: {:?}, Bid: {:?}, Ask: {:?}, Volume: {:?}",
                        event.symbol,
                        event.ticker.last,
                        event.ticker.bid,
                        event.ticker.ask,
                        event.ticker.base_volume
                    );
                    count += 1;
                    if count >= 3 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    // Wait for ticker task
    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), ticker_task).await;

    // Example 2: Watch order book
    println!("\nExample 2: Watching BTC/USD order book...");
    let client = ExmoWs::new(ExchangeConfig::default());
    let mut book_rx = client.watch_order_book("BTC/USD", None).await?;

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
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), book_task).await;

    // Example 3: Watch trades
    println!("\nExample 3: Watching BTC/USD trades...");
    let client = ExmoWs::new(ExchangeConfig::default());
    let mut trade_rx = client.watch_trades("BTC/USD").await?;

    let trade_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = trade_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Trade stream connected"),
                WsMessage::Trade(event) => {
                    for trade in &event.trades {
                        println!(
                            "Trade: {} {} @ {} (amount: {})",
                            event.symbol,
                            trade.side.as_ref().unwrap_or(&"unknown".to_string()),
                            trade.price,
                            trade.amount
                        );
                        count += 1;
                        if count >= 5 {
                            break;
                        }
                    }
                    if count >= 5 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(20), trade_task).await;

    // Example 4: Watch multiple tickers
    println!("\nExample 4: Watching multiple tickers (BTC/USD, ETH/USD)...");
    let client = ExmoWs::new(ExchangeConfig::default());
    let symbols: &[&str] = &["BTC/USD", "ETH/USD"];
    let mut tickers_rx = client.watch_tickers(symbols).await?;

    let tickers_task = tokio::spawn(async move {
        let mut count = 0;
        while let Some(msg) = tickers_rx.recv().await {
            match msg {
                WsMessage::Connected => println!("✓ Multi-ticker stream connected"),
                WsMessage::Ticker(event) => {
                    println!(
                        "Ticker: {} - Last: {:?}, Bid: {:?}, Ask: {:?}",
                        event.symbol, event.ticker.last, event.ticker.bid, event.ticker.ask
                    );
                    count += 1;
                    if count >= 5 {
                        break;
                    }
                },
                WsMessage::Error(err) => eprintln!("Error: {err}"),
                _ => {},
            }
        }
    });

    let _ = tokio::time::timeout(tokio::time::Duration::from_secs(15), tickers_task).await;

    println!("\nExample completed!");

    Ok(())
}
