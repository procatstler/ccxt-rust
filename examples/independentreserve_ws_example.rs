//! IndependentReserve WebSocket Example
//!
//! Demonstrates real-time market data streaming from IndependentReserve

use ccxt_rust::exchanges::IndependentReserveWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("IndependentReserve WebSocket Example");
    println!("=====================================\n");

    // Create WebSocket client
    let mut ws = IndependentReserveWs::new();

    println!("Connecting to IndependentReserve WebSocket...");
    ws.ws_connect().await?;

    // Subscribe to BTC/AUD trades
    println!("\nSubscribing to BTC/AUD trades...");
    let mut trades_rx = ws.watch_trades("BTC/AUD").await?;

    // Subscribe to ETH/AUD orderbook
    println!("Subscribing to ETH/AUD orderbook (depth: 10)...");
    let mut orderbook_rx = ws.watch_order_book("ETH/AUD", Some(10)).await?;

    println!("\nWaiting for messages (press Ctrl+C to stop)...\n");

    // Process messages
    tokio::select! {
        _ = async {
            while let Some(msg) = trades_rx.recv().await {
                if let WsMessage::Trade(event) = msg {
                    println!("Trade: {}", event.symbol);
                    for trade in &event.trades {
                        println!("  ID: {}", trade.id);
                        println!("  Price: {}", trade.price);
                        println!("  Amount: {}", trade.amount);
                        println!("  Side: {:?}", trade.side);
                        println!("  Time: {:?}", trade.datetime);
                        println!();
                    }
                }
            }
        } => {},
        _ = async {
            while let Some(msg) = orderbook_rx.recv().await {
                if let WsMessage::OrderBook(event) = msg {
                    let ob = &event.order_book;
                    println!("OrderBook: {} ({})", event.symbol, if event.is_snapshot { "snapshot" } else { "delta" });

                    // Display top 5 bids and asks
                    println!("  Top 5 Bids:");
                    for (i, bid) in ob.bids.iter().take(5).enumerate() {
                        println!("    {}: {} @ {}", i+1, bid.amount, bid.price);
                    }

                    println!("  Top 5 Asks:");
                    for (i, ask) in ob.asks.iter().take(5).enumerate() {
                        println!("    {}: {} @ {}", i+1, ask.amount, ask.price);
                    }

                    if let Some(spread) = ob.spread() {
                        println!("  Spread: {spread}");
                    }
                    println!();
                }
            }
        } => {},
    }

    // Close connection
    println!("\nClosing WebSocket connection...");
    ws.ws_close().await?;

    Ok(())
}
