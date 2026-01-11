//! Integration tests for the CCXT type system
//!
//! Tests for Market, Order, Ticker, OrderBook, Trade, Balance, and other core types

use ccxt_rust::types::*;
use rust_decimal_macros::dec;

// === Market Type Tests ===

#[test]
fn test_market_spot_creation() {
    let market = Market::spot(
        "BTCUSDT".into(),
        "BTC/USDT".into(),
        "BTC".into(),
        "USDT".into(),
    );

    assert_eq!(market.symbol, "BTC/USDT");
    assert_eq!(market.base, "BTC");
    assert_eq!(market.quote, "USDT");
    assert!(market.spot);
    assert!(market.active);
    assert_eq!(market.market_type, MarketType::Spot);
}

#[test]
fn test_market_type_variants() {
    assert!(matches!(MarketType::Spot, MarketType::Spot));
    assert!(matches!(MarketType::Margin, MarketType::Margin));
    assert!(matches!(MarketType::Swap, MarketType::Swap));
    assert!(matches!(MarketType::Future, MarketType::Future));
    assert!(matches!(MarketType::Option, MarketType::Option));
}

#[test]
fn test_market_limits() {
    let limits = MarketLimits {
        price: MinMax {
            min: Some(dec!(0.01)),
            max: Some(dec!(1000000)),
        },
        amount: MinMax {
            min: Some(dec!(0.0001)),
            max: Some(dec!(1000)),
        },
        cost: MinMax {
            min: Some(dec!(1)),
            max: Some(dec!(10000000)),
        },
        leverage: MinMax::default(),
    };

    assert_eq!(limits.price.min, Some(dec!(0.01)));
    assert_eq!(limits.amount.min, Some(dec!(0.0001)));
}

#[test]
fn test_market_precision() {
    let precision = MarketPrecision {
        price: Some(2),
        amount: Some(4),
        cost: Some(2),
        base: Some(8),
        quote: Some(8),
    };

    assert_eq!(precision.price, Some(2));
    assert_eq!(precision.amount, Some(4));
}

#[test]
fn test_min_max_type() {
    let min_max = MinMax {
        min: Some(dec!(0.01)),
        max: Some(dec!(100)),
    };

    assert_eq!(min_max.min, Some(dec!(0.01)));
    assert_eq!(min_max.max, Some(dec!(100)));
}

// === Order Type Tests ===

#[test]
fn test_order_creation() {
    let order = Order::new(
        "12345".into(),
        "BTC/USDT".into(),
        OrderType::Limit,
        OrderSide::Buy,
        dec!(1),
    );

    assert_eq!(order.id, "12345");
    assert_eq!(order.status, OrderStatus::Open);
    assert_eq!(order.side, OrderSide::Buy);
    assert_eq!(order.order_type, OrderType::Limit);
    assert_eq!(order.symbol, "BTC/USDT");
    assert_eq!(order.amount, dec!(1));
}

#[test]
fn test_order_status_variants() {
    let statuses = vec![
        OrderStatus::Open,
        OrderStatus::Closed,
        OrderStatus::Canceled,
        OrderStatus::Expired,
        OrderStatus::Rejected,
    ];

    for status in statuses {
        // Verify all variants can be matched
        match status {
            OrderStatus::Open => assert!(true),
            OrderStatus::Closed => assert!(true),
            OrderStatus::Canceled => assert!(true),
            OrderStatus::Expired => assert!(true),
            OrderStatus::Rejected => assert!(true),
        }
    }
}

#[test]
fn test_order_type_variants() {
    // Test all OrderType variants
    let types = vec![
        OrderType::Limit,
        OrderType::Market,
        OrderType::StopLimit,
        OrderType::StopMarket,
        OrderType::StopLoss,
        OrderType::StopLossLimit,
        OrderType::TakeProfit,
        OrderType::TakeProfitLimit,
        OrderType::TakeProfitMarket,
        OrderType::LimitMaker,
        OrderType::TrailingStopMarket,
    ];

    for order_type in types {
        match order_type {
            OrderType::Limit => assert!(true),
            OrderType::Market => assert!(true),
            OrderType::StopLimit => assert!(true),
            OrderType::StopMarket => assert!(true),
            OrderType::StopLoss => assert!(true),
            OrderType::StopLossLimit => assert!(true),
            OrderType::TakeProfit => assert!(true),
            OrderType::TakeProfitLimit => assert!(true),
            OrderType::TakeProfitMarket => assert!(true),
            OrderType::LimitMaker => assert!(true),
            OrderType::TrailingStopMarket => assert!(true),
        }
    }
}

#[test]
fn test_order_side_variants() {
    assert!(matches!(OrderSide::Buy, OrderSide::Buy));
    assert!(matches!(OrderSide::Sell, OrderSide::Sell));
}

#[test]
fn test_time_in_force_variants() {
    let tifs = vec![
        TimeInForce::GTC, // Good Till Canceled
        TimeInForce::GTT, // Good Till Time
        TimeInForce::IOC, // Immediate Or Cancel
        TimeInForce::FOK, // Fill Or Kill
        TimeInForce::PO,  // Post Only
    ];

    for tif in tifs {
        match tif {
            TimeInForce::GTC => assert!(true),
            TimeInForce::GTT => assert!(true),
            TimeInForce::IOC => assert!(true),
            TimeInForce::FOK => assert!(true),
            TimeInForce::PO => assert!(true),
        }
    }
}

// === Ticker Type Tests ===

#[test]
fn test_ticker_creation() {
    let ticker = Ticker {
        symbol: "BTC/USDT".into(),
        timestamp: Some(1704067200000),
        datetime: Some("2024-01-01T00:00:00Z".into()),
        high: Some(dec!(45000)),
        low: Some(dec!(42000)),
        bid: Some(dec!(43500)),
        ask: Some(dec!(43550)),
        vwap: Some(dec!(43750)),
        open: Some(dec!(42500)),
        close: Some(dec!(43525)),
        last: Some(dec!(43525)),
        previous_close: Some(dec!(42500)),
        change: Some(dec!(1025)),
        percentage: Some(dec!(2.41)),
        base_volume: Some(dec!(10000)),
        quote_volume: Some(dec!(435000000)),
        ..Default::default()
    };

    assert_eq!(ticker.symbol, "BTC/USDT");
    assert_eq!(ticker.bid, Some(dec!(43500)));
    assert_eq!(ticker.ask, Some(dec!(43550)));
    assert_eq!(ticker.last, Some(dec!(43525)));
}

// === OrderBook Type Tests ===

#[test]
fn test_orderbook_creation() {
    let mut order_book = OrderBook::new("BTC/USDT".into()).with_timestamp(1704067200000);

    order_book.add_bid(dec!(43500), dec!(1.5));
    order_book.add_bid(dec!(43450), dec!(2.0));
    order_book.add_bid(dec!(43400), dec!(3.5));
    order_book.add_ask(dec!(43550), dec!(1.0));
    order_book.add_ask(dec!(43600), dec!(2.5));
    order_book.add_ask(dec!(43650), dec!(4.0));

    assert_eq!(order_book.symbol, "BTC/USDT");
    assert_eq!(order_book.bids.len(), 3);
    assert_eq!(order_book.asks.len(), 3);

    // Best bid should be highest
    assert_eq!(order_book.best_bid().unwrap().price, dec!(43500));
    // Best ask should be lowest
    assert_eq!(order_book.best_ask().unwrap().price, dec!(43550));
}

#[test]
fn test_orderbook_entry() {
    let entry = OrderBookEntry::new(dec!(43500), dec!(1.5));

    assert_eq!(entry.price, dec!(43500));
    assert_eq!(entry.amount, dec!(1.5));
    assert_eq!(entry.cost(), dec!(65250));
}

#[test]
fn test_orderbook_entry_from_array() {
    let entry: OrderBookEntry = [dec!(43500), dec!(1.5)].into();

    assert_eq!(entry.price, dec!(43500));
    assert_eq!(entry.amount, dec!(1.5));
}

#[test]
fn test_orderbook_spread() {
    let mut order_book = OrderBook::new("BTC/USDT".into());
    order_book.add_bid(dec!(43500), dec!(1.0));
    order_book.add_ask(dec!(43550), dec!(1.0));

    assert_eq!(order_book.spread(), Some(dec!(50)));
}

// === Trade Type Tests ===

#[test]
fn test_trade_creation() {
    let trade = Trade::new("trade123".into(), "BTC/USDT".into(), dec!(43525), dec!(0.5));

    assert_eq!(trade.id, "trade123");
    assert_eq!(trade.symbol, "BTC/USDT");
    assert_eq!(trade.price, dec!(43525));
    assert_eq!(trade.amount, dec!(0.5));
    // cost is automatically calculated as price * amount
    assert_eq!(trade.cost, Some(dec!(21762.5)));
}

// === Balance Type Tests ===

#[test]
fn test_balance_creation() {
    let balance = Balance {
        free: Some(dec!(1000)),
        used: Some(dec!(500)),
        total: Some(dec!(1500)),
        debt: Some(dec!(0)),
    };

    assert_eq!(balance.free, Some(dec!(1000)));
    assert_eq!(balance.used, Some(dec!(500)));
    assert_eq!(balance.total, Some(dec!(1500)));
}

#[test]
fn test_balances_collection() {
    let mut balances = Balances::new();

    balances.add(
        "BTC",
        Balance {
            free: Some(dec!(1.5)),
            used: Some(dec!(0.5)),
            total: Some(dec!(2.0)),
            debt: None,
        },
    );

    balances.add(
        "USDT",
        Balance {
            free: Some(dec!(10000)),
            used: Some(dec!(5000)),
            total: Some(dec!(15000)),
            debt: None,
        },
    );

    assert_eq!(balances.free("BTC"), Some(dec!(1.5)));
    assert_eq!(balances.total("USDT"), Some(dec!(15000)));
}

// === OHLCV Type Tests ===

#[test]
fn test_ohlcv_creation() {
    let ohlcv = OHLCV {
        timestamp: 1704067200000,
        open: dec!(42500),
        high: dec!(45000),
        low: dec!(42000),
        close: dec!(43525),
        volume: dec!(10000),
    };

    assert_eq!(ohlcv.open, dec!(42500));
    assert_eq!(ohlcv.high, dec!(45000));
    assert_eq!(ohlcv.low, dec!(42000));
    assert_eq!(ohlcv.close, dec!(43525));
    assert_eq!(ohlcv.volume, dec!(10000));
}

// === Position Type Tests (Futures) ===

#[test]
fn test_position_creation() {
    let position = Position::new("BTC/USDT:USDT")
        .with_side(PositionSide::Long)
        .with_contracts(dec!(1))
        .with_entry_price(dec!(43000))
        .with_leverage(dec!(10))
        .with_margin_mode(MarginMode::Cross);

    assert_eq!(position.symbol, "BTC/USDT:USDT");
    assert_eq!(position.side, Some(PositionSide::Long));
    assert_eq!(position.entry_price, Some(dec!(43000)));
    assert_eq!(position.leverage, Some(dec!(10)));
    assert_eq!(position.margin_mode, Some(MarginMode::Cross));
}

#[test]
fn test_position_side_variants() {
    assert!(matches!(PositionSide::Long, PositionSide::Long));
    assert!(matches!(PositionSide::Short, PositionSide::Short));
    assert!(matches!(PositionSide::Unknown, PositionSide::Unknown));
}

#[test]
fn test_margin_mode_variants() {
    assert!(matches!(MarginMode::Cross, MarginMode::Cross));
    assert!(matches!(MarginMode::Isolated, MarginMode::Isolated));
}

// === Funding Rate Type Tests ===

#[test]
fn test_funding_rate_creation() {
    let funding = FundingRate::new("BTC/USDT:USDT")
        .with_funding_rate(dec!(0.0001))
        .with_timestamp(1704067200000);

    assert_eq!(funding.symbol, "BTC/USDT:USDT");
    assert_eq!(funding.funding_rate, Some(dec!(0.0001)));
    assert_eq!(funding.timestamp, Some(1704067200000));
}

// === Fee Type Tests ===

#[test]
fn test_fee_creation() {
    let fee = Fee {
        currency: Some("USDT".into()),
        cost: Some(dec!(1.25)),
        rate: Some(dec!(0.001)),
    };

    assert_eq!(fee.currency, Some("USDT".into()));
    assert_eq!(fee.cost, Some(dec!(1.25)));
    assert_eq!(fee.rate, Some(dec!(0.001)));
}

#[test]
fn test_trading_fee_creation() {
    let trading_fee = TradingFee {
        symbol: "BTC/USDT".into(),
        maker: dec!(0.001),
        taker: dec!(0.001),
        percentage: true,
        tier_based: true,
        info: serde_json::Value::Null,
    };

    assert_eq!(trading_fee.maker, dec!(0.001));
    assert_eq!(trading_fee.taker, dec!(0.001));
    assert!(trading_fee.percentage);
}

// === Currency Type Tests ===

#[test]
fn test_currency_creation() {
    let currency = Currency::new("BTC".into(), "BTC".into())
        .with_name("Bitcoin")
        .with_fee(dec!(0.0005));

    assert_eq!(currency.code, "BTC");
    assert_eq!(currency.name, Some("Bitcoin".into()));
    assert!(currency.active);
    assert_eq!(currency.fee, Some(dec!(0.0005)));
}

// === Transaction Type Tests ===

#[test]
fn test_transaction_deposit() {
    let transaction = Transaction::deposit("tx123".into(), "BTC".into(), dec!(1))
        .with_timestamp(1704067200000)
        .with_address("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2".into(), None)
        .with_status(TransactionStatus::Ok);

    assert_eq!(transaction.id, "tx123");
    assert!(transaction.is_deposit());
    assert!(transaction.is_completed());
}

#[test]
fn test_transaction_withdrawal() {
    let transaction = Transaction::withdrawal("tx456".into(), "ETH".into(), dec!(10))
        .with_address("0x1234567890abcdef".into(), None)
        .with_txid("0xabcdef123456".into());

    assert_eq!(transaction.id, "tx456");
    assert!(transaction.is_withdrawal());
    assert!(transaction.is_pending());
}

#[test]
fn test_transaction_type_variants() {
    assert!(matches!(TransactionType::Deposit, TransactionType::Deposit));
    assert!(matches!(
        TransactionType::Withdrawal,
        TransactionType::Withdrawal
    ));
}

#[test]
fn test_transaction_status_variants() {
    let statuses = vec![
        TransactionStatus::Pending,
        TransactionStatus::Ok,
        TransactionStatus::Failed,
        TransactionStatus::Canceled,
    ];

    for status in statuses {
        match status {
            TransactionStatus::Pending => assert!(true),
            TransactionStatus::Ok => assert!(true),
            TransactionStatus::Failed => assert!(true),
            TransactionStatus::Canceled => assert!(true),
        }
    }
}

// === ExchangeId Tests ===

#[test]
fn test_exchange_id_as_str() {
    assert_eq!(ExchangeId::Binance.as_str(), "binance");
    assert_eq!(ExchangeId::Kraken.as_str(), "kraken");
    assert_eq!(ExchangeId::Coinbase.as_str(), "coinbase");
    assert_eq!(ExchangeId::Okx.as_str(), "okx");
    assert_eq!(ExchangeId::Bybit.as_str(), "bybit");
    assert_eq!(ExchangeId::Hyperliquid.as_str(), "hyperliquid");
    assert_eq!(ExchangeId::Upbit.as_str(), "upbit");
    assert_eq!(ExchangeId::Bithumb.as_str(), "bithumb");
}

#[test]
fn test_exchange_id_display() {
    assert_eq!(format!("{}", ExchangeId::Binance), "binance");
    assert_eq!(format!("{}", ExchangeId::Kraken), "kraken");
    assert_eq!(format!("{}", ExchangeId::Hyperliquid), "hyperliquid");
}

// === Timeframe Tests ===

#[test]
fn test_timeframe_as_str() {
    assert_eq!(Timeframe::Second1.as_str(), "1s");
    assert_eq!(Timeframe::Minute1.as_str(), "1m");
    assert_eq!(Timeframe::Minute5.as_str(), "5m");
    assert_eq!(Timeframe::Minute15.as_str(), "15m");
    assert_eq!(Timeframe::Hour1.as_str(), "1h");
    assert_eq!(Timeframe::Hour4.as_str(), "4h");
    assert_eq!(Timeframe::Day1.as_str(), "1d");
    assert_eq!(Timeframe::Week1.as_str(), "1w");
    assert_eq!(Timeframe::Month1.as_str(), "1M");
}

#[test]
fn test_timeframe_to_millis() {
    assert_eq!(Timeframe::Second1.to_millis(), 1000);
    assert_eq!(Timeframe::Minute1.to_millis(), 60_000);
    assert_eq!(Timeframe::Minute5.to_millis(), 300_000);
    assert_eq!(Timeframe::Hour1.to_millis(), 3_600_000);
    assert_eq!(Timeframe::Day1.to_millis(), 86_400_000);
    assert_eq!(Timeframe::Week1.to_millis(), 604_800_000);
}

// === ExchangeFeatures Tests ===

#[test]
fn test_exchange_features_default() {
    let features = ExchangeFeatures::default();

    // Default should be all false
    assert!(!features.spot);
    assert!(!features.margin);
    assert!(!features.swap);
    assert!(!features.fetch_ticker);
    assert!(!features.create_order);
}

#[test]
fn test_exchange_features_custom() {
    let features = ExchangeFeatures {
        spot: true,
        margin: true,
        swap: true,
        future: true,
        fetch_ticker: true,
        fetch_order_book: true,
        fetch_trades: true,
        fetch_ohlcv: true,
        create_order: true,
        cancel_order: true,
        fetch_balance: true,
        ws: true,
        watch_ticker: true,
        watch_order_book: true,
        ..Default::default()
    };

    assert!(features.spot);
    assert!(features.margin);
    assert!(features.swap);
    assert!(features.future);
    assert!(features.ws);
}

// === BidAsk Type Tests ===

#[test]
fn test_bid_ask_creation() {
    let bid_ask = BidAsk {
        symbol: "BTC/USDT".into(),
        bid: Some(dec!(43500)),
        bid_volume: Some(dec!(1.5)),
        ask: Some(dec!(43550)),
        ask_volume: Some(dec!(2.0)),
        timestamp: Some(1704067200000),
        datetime: Some("2024-01-01T00:00:00Z".into()),
        info: serde_json::Value::Null,
    };

    assert_eq!(bid_ask.symbol, "BTC/USDT");
    assert_eq!(bid_ask.bid, Some(dec!(43500)));
    assert_eq!(bid_ask.ask, Some(dec!(43550)));
}

// === Leverage Type Tests ===

#[test]
fn test_leverage_creation() {
    let leverage = Leverage::new("BTC/USDT:USDT", MarginMode::Cross, dec!(10), dec!(10));

    assert_eq!(leverage.symbol, "BTC/USDT:USDT");
    assert_eq!(leverage.margin_mode, MarginMode::Cross);
    assert_eq!(leverage.long_leverage, dec!(10));
    assert_eq!(leverage.short_leverage, dec!(10));
}

#[test]
fn test_leverage_tier_creation() {
    let tier = LeverageTier {
        tier: Some(1),
        symbol: Some("BTC/USDT:USDT".into()),
        currency: Some("USDT".into()),
        min_notional: Some(dec!(0)),
        max_notional: Some(dec!(50000)),
        maintenance_margin_rate: Some(dec!(0.005)),
        max_leverage: Some(dec!(125)),
        info: serde_json::Value::Null,
    };

    assert_eq!(tier.tier, Some(1));
    assert_eq!(tier.max_leverage, Some(dec!(125)));
}
