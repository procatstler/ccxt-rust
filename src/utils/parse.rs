//! Parse utilities for common data transformations
//!
//! CCXT의 parse_* 헬퍼 함수들을 Rust로 구현

use chrono::{DateTime, NaiveDateTime};
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::types::{OrderSide, OrderStatus, OrderType, Timeframe};

/// Parse a number from various formats
pub fn parse_number<T: FromStr>(value: &str) -> Option<T> {
    let cleaned = value.trim().replace(',', "");
    cleaned.parse().ok()
}

/// Parse Decimal from string with fallback
pub fn parse_decimal(value: &str) -> Option<Decimal> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return None;
    }
    Decimal::from_str(cleaned).ok()
}

/// Parse Decimal with default value
pub fn parse_decimal_or(value: &str, default: Decimal) -> Decimal {
    parse_decimal(value).unwrap_or(default)
}

/// Parse i64 from string
pub fn parse_i64(value: &str) -> Option<i64> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse().ok()
}

/// Parse i64 with default value
pub fn parse_i64_or(value: &str, default: i64) -> i64 {
    parse_i64(value).unwrap_or(default)
}

/// Parse f64 from string
pub fn parse_f64(value: &str) -> Option<f64> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse().ok()
}

/// Parse f64 with default value
pub fn parse_f64_or(value: &str, default: f64) -> f64 {
    parse_f64(value).unwrap_or(default)
}

/// Convert timeframe string to seconds
///
/// Supports: 1s, 1m, 5m, 15m, 30m, 1h, 2h, 4h, 6h, 8h, 12h, 1d, 3d, 1w, 1M
pub fn parse_timeframe_to_seconds(timeframe: &str) -> Option<i64> {
    let tf = timeframe.trim().to_lowercase();

    if tf.is_empty() {
        return None;
    }

    // Extract numeric part and unit
    let (num_str, unit) = tf.split_at(tf.len().saturating_sub(1));
    let num: i64 = if num_str.is_empty() {
        1
    } else {
        num_str.parse().ok()?
    };

    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => return None,
    };

    Some(num * multiplier)
}

/// Convert seconds to timeframe string
pub fn seconds_to_timeframe(seconds: i64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else if seconds < 604800 {
        format!("{}d", seconds / 86400)
    } else {
        format!("{}w", seconds / 604800)
    }
}

/// Convert Timeframe enum to seconds
pub fn timeframe_to_seconds(timeframe: &Timeframe) -> i64 {
    match timeframe {
        Timeframe::Second1 => 1,
        Timeframe::Minute1 => 60,
        Timeframe::Minute3 => 180,
        Timeframe::Minute5 => 300,
        Timeframe::Minute15 => 900,
        Timeframe::Minute30 => 1800,
        Timeframe::Hour1 => 3600,
        Timeframe::Hour2 => 7200,
        Timeframe::Hour3 => 10800,
        Timeframe::Hour4 => 14400,
        Timeframe::Hour6 => 21600,
        Timeframe::Hour8 => 28800,
        Timeframe::Hour12 => 43200,
        Timeframe::Day1 => 86400,
        Timeframe::Day3 => 259200,
        Timeframe::Week1 => 604800,
        Timeframe::Month1 => 2592000, // 30 days approximation
    }
}

/// Parse ISO 8601 datetime string to timestamp (milliseconds)
pub fn parse_iso8601(datetime: &str) -> Option<i64> {
    // Try RFC 3339 first (most common)
    if let Ok(dt) = DateTime::parse_from_rfc3339(datetime) {
        return Some(dt.timestamp_millis());
    }

    // Try without timezone (assume UTC)
    if let Ok(dt) = NaiveDateTime::parse_from_str(datetime, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc().timestamp_millis());
    }

    // Try with milliseconds
    if let Ok(dt) = NaiveDateTime::parse_from_str(datetime, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc().timestamp_millis());
    }

    // Try date only
    if let Ok(dt) = NaiveDateTime::parse_from_str(&format!("{}T00:00:00", datetime), "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.and_utc().timestamp_millis());
    }

    None
}

/// Convert timestamp (milliseconds) to ISO 8601 string
pub fn timestamp_to_iso8601(timestamp_ms: i64) -> String {
    DateTime::from_timestamp_millis(timestamp_ms)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

/// Parse date string in various formats to timestamp (milliseconds)
pub fn parse_date(date: &str) -> Option<i64> {
    let trimmed = date.trim();

    // Try ISO 8601
    if let Some(ts) = parse_iso8601(trimmed) {
        return Some(ts);
    }

    // Try Unix timestamp (seconds)
    if let Ok(ts) = trimmed.parse::<i64>() {
        // If it looks like milliseconds (13+ digits), return as is
        if ts > 1_000_000_000_000 {
            return Some(ts);
        }
        // Otherwise treat as seconds
        return Some(ts * 1000);
    }

    // Try common date formats
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
        "%d-%m-%Y %H:%M:%S",
        "%Y-%m-%d",
        "%Y/%m/%d",
    ];

    for fmt in &formats {
        if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }

    None
}

/// Parse order side from string
pub fn parse_order_side(side: &str) -> Option<OrderSide> {
    match side.to_lowercase().as_str() {
        "buy" | "bid" | "long" => Some(OrderSide::Buy),
        "sell" | "ask" | "short" => Some(OrderSide::Sell),
        _ => None,
    }
}

/// Parse order side with default
pub fn parse_order_side_or(side: &str, default: OrderSide) -> OrderSide {
    parse_order_side(side).unwrap_or(default)
}

/// Parse order status from string
pub fn parse_order_status(status: &str) -> Option<OrderStatus> {
    match status.to_lowercase().as_str() {
        "open" | "new" | "active" | "pending" | "live" => Some(OrderStatus::Open),
        "closed" | "filled" | "completed" | "done" => Some(OrderStatus::Closed),
        "canceled" | "cancelled" | "revoked" => Some(OrderStatus::Canceled),
        "expired" => Some(OrderStatus::Expired),
        "rejected" => Some(OrderStatus::Rejected),
        _ => None,
    }
}

/// Parse order status with default
pub fn parse_order_status_or(status: &str, default: OrderStatus) -> OrderStatus {
    parse_order_status(status).unwrap_or(default)
}

/// Parse order type from string
pub fn parse_order_type(order_type: &str) -> Option<OrderType> {
    match order_type.to_lowercase().as_str() {
        "market" => Some(OrderType::Market),
        "limit" => Some(OrderType::Limit),
        "stop" | "stop_market" | "stop-market" => Some(OrderType::StopMarket),
        "stop_limit" | "stop-limit" | "stoplimit" => Some(OrderType::StopLimit),
        "stop_loss" | "stop-loss" | "stoploss" => Some(OrderType::StopLoss),
        "stop_loss_limit" | "stop-loss-limit" => Some(OrderType::StopLossLimit),
        "take_profit" | "take-profit" | "takeprofit" | "tp" => Some(OrderType::TakeProfit),
        "take_profit_limit" | "take-profit-limit" => Some(OrderType::TakeProfitLimit),
        "take_profit_market" | "take-profit-market" => Some(OrderType::TakeProfitMarket),
        "limit_maker" | "limit-maker" | "limitmaker" | "maker" => Some(OrderType::LimitMaker),
        "trailing_stop" | "trailing-stop" | "trailing_stop_market" => Some(OrderType::TrailingStopMarket),
        _ => None,
    }
}

/// Parse order type with default
pub fn parse_order_type_or(order_type: &str, default: OrderType) -> OrderType {
    parse_order_type(order_type).unwrap_or(default)
}

/// Parse market symbol to base and quote currencies
///
/// Examples: "BTC/USDT" -> ("BTC", "USDT"), "BTCUSDT" -> ("BTC", "USDT")
pub fn parse_symbol(symbol: &str) -> Option<(String, String)> {
    let trimmed = symbol.trim().to_uppercase();

    // Try slash separator first
    if trimmed.contains('/') {
        let parts: Vec<&str> = trimmed.split('/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Try common quote currencies
    let quote_currencies = ["USDT", "USDC", "BUSD", "USD", "EUR", "BTC", "ETH", "BNB"];

    for quote in &quote_currencies {
        if trimmed.ends_with(quote) && trimmed.len() > quote.len() {
            let base = &trimmed[..trimmed.len() - quote.len()];
            return Some((base.to_string(), quote.to_string()));
        }
    }

    None
}

/// Format market symbol with separator
pub fn format_symbol(base: &str, quote: &str, separator: &str) -> String {
    format!("{}{}{}", base.to_uppercase(), separator, quote.to_uppercase())
}

/// Parse percentage string to Decimal
///
/// Examples: "5.5%", "5.5", "-2.3%" -> Decimal
pub fn parse_percentage(value: &str) -> Option<Decimal> {
    let cleaned = value.trim().trim_end_matches('%').trim();
    parse_decimal(cleaned)
}

/// Parse boolean from various string representations
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.to_lowercase().trim() {
        "true" | "1" | "yes" | "on" | "enabled" => Some(true),
        "false" | "0" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

/// Parse boolean with default
pub fn parse_bool_or(value: &str, default: bool) -> bool {
    parse_bool(value).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_parse_decimal() {
        assert_eq!(parse_decimal("123.456"), Some(dec!(123.456)));
        assert_eq!(parse_decimal("  456.789  "), Some(dec!(456.789)));
        assert_eq!(parse_decimal(""), None);
        assert_eq!(parse_decimal("invalid"), None);
    }

    #[test]
    fn test_parse_timeframe_to_seconds() {
        assert_eq!(parse_timeframe_to_seconds("1m"), Some(60));
        assert_eq!(parse_timeframe_to_seconds("5m"), Some(300));
        assert_eq!(parse_timeframe_to_seconds("1h"), Some(3600));
        assert_eq!(parse_timeframe_to_seconds("4h"), Some(14400));
        assert_eq!(parse_timeframe_to_seconds("1d"), Some(86400));
        assert_eq!(parse_timeframe_to_seconds("1w"), Some(604800));
    }

    #[test]
    fn test_seconds_to_timeframe() {
        assert_eq!(seconds_to_timeframe(60), "1m");
        assert_eq!(seconds_to_timeframe(300), "5m");
        assert_eq!(seconds_to_timeframe(3600), "1h");
        assert_eq!(seconds_to_timeframe(86400), "1d");
    }

    #[test]
    fn test_parse_iso8601() {
        let ts = parse_iso8601("2023-11-15T10:30:00Z");
        assert!(ts.is_some());

        let ts2 = parse_iso8601("2023-11-15T10:30:00+00:00");
        assert!(ts2.is_some());
    }

    #[test]
    fn test_parse_order_side() {
        assert_eq!(parse_order_side("buy"), Some(OrderSide::Buy));
        assert_eq!(parse_order_side("SELL"), Some(OrderSide::Sell));
        assert_eq!(parse_order_side("bid"), Some(OrderSide::Buy));
        assert_eq!(parse_order_side("ask"), Some(OrderSide::Sell));
        assert_eq!(parse_order_side("invalid"), None);
    }

    #[test]
    fn test_parse_order_status() {
        assert_eq!(parse_order_status("open"), Some(OrderStatus::Open));
        assert_eq!(parse_order_status("filled"), Some(OrderStatus::Closed));
        assert_eq!(parse_order_status("canceled"), Some(OrderStatus::Canceled));
        assert_eq!(parse_order_status("cancelled"), Some(OrderStatus::Canceled));
    }

    #[test]
    fn test_parse_order_type() {
        assert_eq!(parse_order_type("market"), Some(OrderType::Market));
        assert_eq!(parse_order_type("limit"), Some(OrderType::Limit));
        assert_eq!(parse_order_type("stop_limit"), Some(OrderType::StopLimit));
    }

    #[test]
    fn test_parse_symbol() {
        assert_eq!(parse_symbol("BTC/USDT"), Some(("BTC".to_string(), "USDT".to_string())));
        assert_eq!(parse_symbol("BTCUSDT"), Some(("BTC".to_string(), "USDT".to_string())));
        assert_eq!(parse_symbol("ETHBTC"), Some(("ETH".to_string(), "BTC".to_string())));
    }

    #[test]
    fn test_parse_percentage() {
        assert_eq!(parse_percentage("5.5%"), Some(dec!(5.5)));
        assert_eq!(parse_percentage("5.5"), Some(dec!(5.5)));
        assert_eq!(parse_percentage("-2.3%"), Some(dec!(-2.3)));
    }

    #[test]
    fn test_parse_bool() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("1"), Some(true));
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("invalid"), None);
    }
}
