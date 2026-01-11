//! Integration tests for WebSocket implementations
//!
//! Tests for WsExchange trait and WebSocket streaming functionality
//!
//! This test file requires the "cex" feature to be enabled for CEX WebSocket tests.

#![cfg(feature = "cex")]

use ccxt_rust::types::{WsMessage, WsOrderBookEvent, WsSubscription, WsTickerEvent, WsTradeEvent};
use ccxt_rust::Timeframe;

// === WsMessage Type Tests ===

#[test]
fn test_ws_message_connected() {
    let msg = WsMessage::Connected;
    assert!(matches!(msg, WsMessage::Connected));
}

#[test]
fn test_ws_message_disconnected() {
    let msg = WsMessage::Disconnected;
    assert!(matches!(msg, WsMessage::Disconnected));
}

#[test]
fn test_ws_message_authenticated() {
    let msg = WsMessage::Authenticated;
    assert!(matches!(msg, WsMessage::Authenticated));
}

#[test]
fn test_ws_message_error() {
    let msg = WsMessage::Error("connection failed".into());
    if let WsMessage::Error(e) = msg {
        assert_eq!(e, "connection failed");
    } else {
        panic!("Expected Error variant");
    }
}

#[test]
fn test_ws_message_subscribed() {
    let msg = WsMessage::Subscribed {
        channel: "ticker".into(),
        symbol: Some("BTC/USDT".into()),
    };

    if let WsMessage::Subscribed { channel, symbol } = msg {
        assert_eq!(channel, "ticker");
        assert_eq!(symbol, Some("BTC/USDT".into()));
    } else {
        panic!("Expected Subscribed variant");
    }
}

#[test]
fn test_ws_message_unsubscribed() {
    let msg = WsMessage::Unsubscribed {
        channel: "orderbook".into(),
        symbol: Some("ETH/USDT".into()),
    };

    if let WsMessage::Unsubscribed { channel, symbol } = msg {
        assert_eq!(channel, "orderbook");
        assert_eq!(symbol, Some("ETH/USDT".into()));
    } else {
        panic!("Expected Unsubscribed variant");
    }
}

// === WsSubscription Tests ===

#[test]
fn test_ws_subscription_creation() {
    let sub = WsSubscription::new("ticker".into(), Some("BTC/USDT".into()));
    assert_eq!(sub.channel(), "ticker");
    assert_eq!(sub.symbol(), Some("BTC/USDT"));
}

#[test]
fn test_ws_subscription_no_symbol() {
    let sub = WsSubscription::new("balance".into(), None);
    assert_eq!(sub.channel(), "balance");
    assert_eq!(sub.symbol(), None);
}

#[test]
fn test_ws_subscription_with_unsubscribe() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let unsubscribed = Arc::new(AtomicBool::new(false));
    let unsubscribed_clone = unsubscribed.clone();

    {
        let _sub = WsSubscription::new("ticker".into(), Some("BTC/USDT".into())).with_unsubscribe(
            move || {
                unsubscribed_clone.store(true, Ordering::SeqCst);
            },
        );
        // Subscription is dropped here
    }

    // Unsubscribe function should have been called on drop
    assert!(unsubscribed.load(Ordering::SeqCst));
}

// === WebSocket Feature Tests for Major Exchanges ===

#[tokio::test]
async fn test_binance_ws_features() {
    use ccxt_rust::exchanges::BinanceWs;
    use ccxt_rust::types::WsExchange;

    let ws = BinanceWs::new();

    // Test initial connection state
    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_bybit_ws_features() {
    use ccxt_rust::exchanges::BybitWs;
    use ccxt_rust::types::WsExchange;

    let ws = BybitWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_okx_ws_features() {
    use ccxt_rust::exchanges::OkxWs;
    use ccxt_rust::types::WsExchange;

    let ws = OkxWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_gate_ws_features() {
    use ccxt_rust::exchanges::GateWs;
    use ccxt_rust::types::WsExchange;

    let ws = GateWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_kraken_ws_features() {
    use ccxt_rust::exchanges::KrakenWs;
    use ccxt_rust::types::WsExchange;

    let ws = KrakenWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_kucoin_ws_features() {
    use ccxt_rust::exchanges::KucoinWs;
    use ccxt_rust::types::WsExchange;

    let ws = KucoinWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_bitget_ws_features() {
    use ccxt_rust::exchanges::BitgetWs;
    use ccxt_rust::types::WsExchange;

    let ws = BitgetWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_mexc_ws_features() {
    use ccxt_rust::exchanges::MexcWs;
    use ccxt_rust::types::WsExchange;

    let ws = MexcWs::new();

    assert!(!ws.ws_is_connected().await);
}

// === DEX WebSocket Tests (requires "dex" feature) ===

#[cfg(feature = "dex")]
#[tokio::test]
async fn test_hyperliquid_ws_features() {
    use ccxt_rust::exchanges::HyperliquidWs;
    use ccxt_rust::types::WsExchange;

    let ws = HyperliquidWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[cfg(feature = "dex")]
#[tokio::test]
async fn test_dydx_ws_features() {
    use ccxt_rust::exchanges::DydxWs;
    use ccxt_rust::types::WsExchange;

    let ws = DydxWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[cfg(feature = "dex")]
#[tokio::test]
async fn test_paradex_ws_features() {
    use ccxt_rust::exchanges::ParadexWs;
    use ccxt_rust::types::WsExchange;

    let ws = ParadexWs::new();

    assert!(!ws.ws_is_connected().await);
}

// === Korean Exchange WebSocket Tests ===

#[tokio::test]
async fn test_upbit_ws_features() {
    use ccxt_rust::exchanges::UpbitWs;
    use ccxt_rust::types::WsExchange;

    let ws = UpbitWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_bithumb_ws_features() {
    use ccxt_rust::exchanges::BithumbWs;
    use ccxt_rust::types::WsExchange;

    let ws = BithumbWs::new();

    assert!(!ws.ws_is_connected().await);
}

// === Futures Exchange WebSocket Tests ===

#[tokio::test]
async fn test_binance_futures_ws_features() {
    use ccxt_rust::exchanges::BinanceFuturesWs;
    use ccxt_rust::types::WsExchange;

    let ws = BinanceFuturesWs::new();

    assert!(!ws.ws_is_connected().await);
}

#[tokio::test]
async fn test_binance_coinm_ws_features() {
    use ccxt_rust::exchanges::BinanceCoinmWs;
    use ccxt_rust::types::WsExchange;

    let ws = BinanceCoinmWs::new();

    assert!(!ws.ws_is_connected().await);
}

// === WebSocket Event Structure Tests ===

#[test]
fn test_ws_ticker_event_structure() {
    use ccxt_rust::types::Ticker;
    use rust_decimal::Decimal;

    let event = WsTickerEvent {
        symbol: "BTC/USDT".into(),
        ticker: Ticker {
            symbol: "BTC/USDT".into(),
            timestamp: Some(1704067200000),
            datetime: Some("2024-01-01T00:00:00Z".into()),
            high: Some(Decimal::from(45000)),
            low: Some(Decimal::from(42000)),
            bid: Some(Decimal::from(43500)),
            ask: Some(Decimal::from(43550)),
            last: Some(Decimal::from(43525)),
            close: Some(Decimal::from(43525)),
            open: Some(Decimal::from(42500)),
            base_volume: Some(Decimal::from(1000)),
            quote_volume: Some(Decimal::from(43000000)),
            ..Default::default()
        },
    };

    assert_eq!(event.symbol, "BTC/USDT");
    assert_eq!(event.ticker.last, Some(Decimal::from(43525)));
}

#[test]
fn test_ws_order_book_event_structure() {
    use ccxt_rust::types::{OrderBook, OrderBookEntry};

    let mut order_book = OrderBook::new("ETH/USDT".into()).with_timestamp(1704067200000);
    order_book.bids.push(OrderBookEntry::new(
        rust_decimal_macros::dec!(2200),
        rust_decimal_macros::dec!(10),
    ));
    order_book.asks.push(OrderBookEntry::new(
        rust_decimal_macros::dec!(2201),
        rust_decimal_macros::dec!(15),
    ));
    order_book.nonce = Some(12345);

    let event = WsOrderBookEvent {
        symbol: "ETH/USDT".into(),
        order_book,
        is_snapshot: true,
    };

    assert_eq!(event.symbol, "ETH/USDT");
    assert!(event.is_snapshot);
    assert_eq!(event.order_book.bids.len(), 1);
    assert_eq!(event.order_book.asks.len(), 1);
}

#[test]
fn test_ws_trade_event_structure() {
    use ccxt_rust::types::Trade;
    use rust_decimal::Decimal;

    let event = WsTradeEvent {
        symbol: "BTC/USDT".into(),
        trades: vec![Trade::new(
            "123456".into(),
            "BTC/USDT".into(),
            Decimal::from(43500),
            Decimal::from(1),
        )],
    };

    assert_eq!(event.symbol, "BTC/USDT");
    assert_eq!(event.trades.len(), 1);
    assert_eq!(event.trades[0].price, Decimal::from(43500));
}

// === Timeframe Tests ===

#[test]
fn test_timeframe_conversion() {
    assert_eq!(Timeframe::Minute1.as_str(), "1m");
    assert_eq!(Timeframe::Minute5.as_str(), "5m");
    assert_eq!(Timeframe::Minute15.as_str(), "15m");
    assert_eq!(Timeframe::Hour1.as_str(), "1h");
    assert_eq!(Timeframe::Hour4.as_str(), "4h");
    assert_eq!(Timeframe::Day1.as_str(), "1d");
    assert_eq!(Timeframe::Week1.as_str(), "1w");
}

#[test]
fn test_timeframe_to_millis() {
    assert_eq!(Timeframe::Second1.to_millis(), 1000);
    assert_eq!(Timeframe::Minute1.to_millis(), 60_000);
    assert_eq!(Timeframe::Hour1.to_millis(), 3_600_000);
    assert_eq!(Timeframe::Day1.to_millis(), 86_400_000);
    assert_eq!(Timeframe::Week1.to_millis(), 604_800_000);
}

// === WebSocket Config Tests ===

#[tokio::test]
async fn test_ws_creation() {
    use ccxt_rust::exchanges::BinanceWs;

    // WebSocket instances can be created without credentials for public streams
    let _ws = BinanceWs::new();
}

// === Exchange WS Feature Flag Tests ===

#[tokio::test]
async fn test_exchange_has_ws_features() {
    use ccxt_rust::exchanges::Binance;
    use ccxt_rust::{Exchange, ExchangeConfig};

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config).unwrap();

    let features = exchange.has();
    assert!(features.ws, "Binance should support WebSocket");
    assert!(features.watch_ticker, "Should support watch_ticker");
    assert!(features.watch_order_book, "Should support watch_order_book");
    assert!(features.watch_trades, "Should support watch_trades");
}
