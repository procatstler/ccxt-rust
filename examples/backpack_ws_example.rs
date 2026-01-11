//! Backpack WebSocket Example
//!
//! Demonstrates using the Backpack exchange WebSocket API
//! Run with: cargo run --example backpack_ws_example

use ccxt_rust::exchanges::BackpackWs;
use ccxt_rust::types::{WsExchange, WsMessage};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Backpack Exchange WebSocket Example");
    println!("====================================\n");

    let ws = BackpackWs::new();
    println!("Exchange: Backpack (backpack)");
    println!();

    // Example 1: Watch ticker
    println!("Subscribing to BTC/USDC ticker...");
    let mut ticker_rx = ws.watch_ticker("BTC/USDC").await?;

    println!("Waiting for ticker updates (5 messages)...\n");
    let mut count = 0;
    let timeout = Duration::from_secs(30);

    loop {
        tokio::select! {
            Some(msg) = ticker_rx.recv() => {
                if let WsMessage::Ticker(event) = msg {
                    println!("Ticker Update:");
                    println!("  Symbol: {}", event.ticker.symbol);
                    println!("  Last: {:?}", event.ticker.last);
                    println!("  Bid: {:?}", event.ticker.bid);
                    println!("  Ask: {:?}", event.ticker.ask);
                    println!("  Volume: {:?}", event.ticker.base_volume);
                    println!();
                    count += 1;
                    if count >= 5 {
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                println!("Timeout waiting for ticker");
                break;
            }
        }
    }

    // Example 2: Watch order book
    println!("\nSubscribing to BTC/USDC order book...");
    let ws2 = BackpackWs::new();
    let mut orderbook_rx = ws2.watch_order_book("BTC/USDC", Some(10)).await?;

    println!("Waiting for order book updates (3 messages)...\n");
    count = 0;

    loop {
        tokio::select! {
            Some(msg) = orderbook_rx.recv() => {
                if let WsMessage::OrderBook(event) = msg {
                    println!("Order Book Update:");
                    println!("  Symbol: {}", event.order_book.symbol);
                    println!("  Bids: {} levels", event.order_book.bids.len());
                    println!("  Asks: {} levels", event.order_book.asks.len());
                    if let Some(bid) = event.order_book.bids.first() {
                        println!("  Best Bid: {} @ {}", bid.amount, bid.price);
                    }
                    if let Some(ask) = event.order_book.asks.first() {
                        println!("  Best Ask: {} @ {}", ask.amount, ask.price);
                    }
                    println!();
                    count += 1;
                    if count >= 3 {
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                println!("Timeout waiting for order book");
                break;
            }
        }
    }

    // Example 3: Watch trades
    println!("\nSubscribing to BTC/USDC trades...");
    let ws3 = BackpackWs::new();
    let mut trades_rx = ws3.watch_trades("BTC/USDC").await?;

    println!("Waiting for trade updates (5 messages)...\n");
    count = 0;

    loop {
        tokio::select! {
            Some(msg) = trades_rx.recv() => {
                if let WsMessage::Trade(event) = msg {
                    for trade in &event.trades {
                        println!("Trade: {} {} {} @ {} = {:?}",
                            trade.datetime.clone().unwrap_or_else(|| "?".to_string()),
                            trade.side.clone().unwrap_or_else(|| "?".to_string()),
                            trade.amount,
                            trade.price,
                            trade.cost
                        );
                    }
                    count += 1;
                    if count >= 5 {
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                println!("Timeout waiting for trades");
                break;
            }
        }
    }

    println!("\nDone!");
    Ok(())
}
