//! HitBTC WebSocket Example
//!
//! Demonstrates real-time data streaming from HitBTC

use ccxt_rust::exchanges::HitbtcWs;
use ccxt_rust::types::{WsExchange, WsMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("HitBTC WebSocket Example");
    println!("========================\n");

    // Create WebSocket client
    let hitbtc_ws = HitbtcWs::new();

    // Watch BTC/USDT ticker
    println!("Subscribing to BTC/USDT ticker...");
    let mut ticker_rx = hitbtc_ws.watch_ticker("BTC/USDT").await?;

    println!("Waiting for ticker updates (Ctrl+C to stop)...\n");

    // Listen for ticker updates
    let mut count = 0;
    while let Some(msg) = ticker_rx.recv().await {
        match msg {
            WsMessage::Connected => {
                println!("✓ Connected to HitBTC WebSocket");
            }
            WsMessage::Ticker(ticker_event) => {
                count += 1;
                println!(
                    "[{}] {} - Last: {:?}, Bid: {:?}, Ask: {:?}, Volume: {:?}",
                    count,
                    ticker_event.symbol,
                    ticker_event.ticker.last,
                    ticker_event.ticker.bid,
                    ticker_event.ticker.ask,
                    ticker_event.ticker.base_volume
                );

                // Stop after 5 updates for demo purposes
                if count >= 5 {
                    println!("\n✓ Received 5 ticker updates. Stopping...");
                    break;
                }
            }
            WsMessage::Error(err) => {
                eprintln!("Error: {}", err);
                break;
            }
            WsMessage::Disconnected => {
                println!("Disconnected from WebSocket");
                break;
            }
            _ => {}
        }
    }

    println!("\nExample completed.");
    Ok(())
}
