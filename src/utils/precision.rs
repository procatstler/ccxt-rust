//! Precision utilities for decimal number formatting
//!
//! CCXT의 decimal_to_precision 함수를 Rust로 구현

#![allow(clippy::manual_strip)]

use rust_decimal::prelude::*;
use rust_decimal::Decimal;

/// Rounding modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingMode {
    /// Round towards positive infinity
    Up,
    /// Round towards negative infinity
    Down,
    /// Round towards zero
    TowardsZero,
    /// Round away from zero
    AwayFromZero,
    /// Round to nearest, ties go to even (banker's rounding)
    HalfEven,
    /// Round to nearest, ties go up
    HalfUp,
    /// Round to nearest, ties go down
    HalfDown,
}

/// Precision modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecisionMode {
    /// Number of decimal places (e.g., 2 means 0.01)
    DecimalPlaces,
    /// Number of significant digits (e.g., 3 means 123, 12.3, 1.23)
    SignificantDigits,
    /// Tick size (e.g., 0.0001 means multiples of 0.0001)
    TickSize,
}

/// Padding modes for string output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaddingMode {
    /// No padding
    NoPadding,
    /// Pad with zeros to specified precision
    PadWithZeros,
}

/// Format a decimal number with specified precision
///
/// # Arguments
/// * `value` - The decimal value to format
/// * `precision` - The precision value (interpretation depends on mode)
/// * `rounding_mode` - How to round the number
/// * `precision_mode` - How to interpret the precision value
/// * `padding_mode` - Whether to pad with zeros
///
/// # Returns
/// Formatted string representation of the number
pub fn decimal_to_precision(
    value: Decimal,
    precision: i32,
    rounding_mode: RoundingMode,
    precision_mode: PrecisionMode,
    padding_mode: PaddingMode,
) -> String {
    match precision_mode {
        PrecisionMode::DecimalPlaces => {
            format_decimal_places(value, precision, rounding_mode, padding_mode)
        },
        PrecisionMode::SignificantDigits => {
            format_significant_digits(value, precision as u32, rounding_mode, padding_mode)
        },
        PrecisionMode::TickSize => {
            let tick = Decimal::new(1, precision as u32);
            format_tick_size(value, tick, rounding_mode, padding_mode)
        },
    }
}

/// Format with tick size precision
pub fn decimal_to_precision_tick(
    value: Decimal,
    tick_size: Decimal,
    rounding_mode: RoundingMode,
    padding_mode: PaddingMode,
) -> String {
    format_tick_size(value, tick_size, rounding_mode, padding_mode)
}

fn format_decimal_places(
    value: Decimal,
    places: i32,
    rounding_mode: RoundingMode,
    padding_mode: PaddingMode,
) -> String {
    let rounded = round_decimal(value, places, rounding_mode);

    if padding_mode == PaddingMode::NoPadding {
        // Remove trailing zeros
        let s = format!("{rounded}");
        remove_trailing_zeros(&s)
    } else {
        // Format with fixed decimal places
        if places <= 0 {
            format!("{}", rounded.trunc())
        } else {
            format!("{:.1$}", rounded, places as usize)
        }
    }
}

fn format_significant_digits(
    value: Decimal,
    digits: u32,
    rounding_mode: RoundingMode,
    padding_mode: PaddingMode,
) -> String {
    if value.is_zero() {
        return "0".to_string();
    }

    let abs_value = value.abs();
    // Calculate floor(log10(abs_value)) manually
    let log10_floor = calculate_log10_floor(abs_value);
    let places = (digits as i32) - 1 - log10_floor;

    let rounded = round_decimal(value, places, rounding_mode);

    if padding_mode == PaddingMode::NoPadding {
        let s = format!("{rounded}");
        remove_trailing_zeros(&s)
    } else if places <= 0 {
        format!("{}", rounded.trunc())
    } else {
        format!("{:.1$}", rounded, places.max(0) as usize)
    }
}

/// Calculate floor(log10(value)) for Decimal
fn calculate_log10_floor(value: Decimal) -> i32 {
    if value.is_zero() {
        return 0;
    }

    let value_str = value.abs().to_string();

    // Find position of decimal point
    if let Some(dot_pos) = value_str.find('.') {
        // Count digits before decimal
        let int_part = &value_str[..dot_pos];
        if int_part == "0" {
            // Value is less than 1, count leading zeros after decimal
            let frac_part = &value_str[dot_pos + 1..];
            let leading_zeros = frac_part.chars().take_while(|&c| c == '0').count();
            -(leading_zeros as i32 + 1)
        } else {
            (int_part.len() as i32) - 1
        }
    } else {
        // No decimal point, it's an integer
        (value_str.trim_start_matches('-').len() as i32) - 1
    }
}

fn format_tick_size(
    value: Decimal,
    tick_size: Decimal,
    rounding_mode: RoundingMode,
    padding_mode: PaddingMode,
) -> String {
    if tick_size.is_zero() {
        return value.to_string();
    }

    let quotient = value / tick_size;
    let rounded_quotient = round_to_integer(quotient, rounding_mode);
    let rounded_value = rounded_quotient * tick_size;

    // Determine decimal places from tick size
    let tick_str = tick_size.to_string();
    let places = if let Some(pos) = tick_str.find('.') {
        tick_str.len() - pos - 1
    } else {
        0
    };

    if padding_mode == PaddingMode::NoPadding {
        let s = format!("{rounded_value}");
        remove_trailing_zeros(&s)
    } else {
        format!("{rounded_value:.places$}")
    }
}

fn round_decimal(value: Decimal, places: i32, mode: RoundingMode) -> Decimal {
    let factor = Decimal::from(10i64.pow(places.unsigned_abs()));

    let scaled = if places >= 0 {
        value * factor
    } else {
        value / factor
    };

    let rounded = round_to_integer(scaled, mode);

    if places >= 0 {
        rounded / factor
    } else {
        rounded * factor
    }
}

fn round_to_integer(value: Decimal, mode: RoundingMode) -> Decimal {
    match mode {
        RoundingMode::Up => value.ceil(),
        RoundingMode::Down => value.floor(),
        RoundingMode::TowardsZero => value.trunc(),
        RoundingMode::AwayFromZero => {
            if value.is_sign_positive() {
                value.ceil()
            } else {
                value.floor()
            }
        },
        RoundingMode::HalfEven => {
            value.round_dp_with_strategy(0, RoundingStrategy::MidpointNearestEven)
        },
        RoundingMode::HalfUp => {
            value.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        },
        RoundingMode::HalfDown => {
            value.round_dp_with_strategy(0, RoundingStrategy::MidpointTowardZero)
        },
    }
}

fn remove_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }

    let trimmed = s.trim_end_matches('0');
    if trimmed.ends_with('.') {
        trimmed[..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Amount to precision - formats amount with market's amount precision
pub fn amount_to_precision(amount: Decimal, precision: i32) -> String {
    decimal_to_precision(
        amount,
        precision,
        RoundingMode::TowardsZero,
        PrecisionMode::DecimalPlaces,
        PaddingMode::NoPadding,
    )
}

/// Price to precision - formats price with market's price precision
pub fn price_to_precision(price: Decimal, precision: i32) -> String {
    decimal_to_precision(
        price,
        precision,
        RoundingMode::HalfUp,
        PrecisionMode::DecimalPlaces,
        PaddingMode::NoPadding,
    )
}

/// Cost to precision - formats cost (price * amount) with market's cost precision
pub fn cost_to_precision(cost: Decimal, precision: i32) -> String {
    decimal_to_precision(
        cost,
        precision,
        RoundingMode::HalfUp,
        PrecisionMode::DecimalPlaces,
        PaddingMode::NoPadding,
    )
}

/// Fee to precision - formats fee with appropriate precision
pub fn fee_to_precision(fee: Decimal, precision: i32) -> String {
    decimal_to_precision(
        fee,
        precision,
        RoundingMode::Up,
        PrecisionMode::DecimalPlaces,
        PaddingMode::NoPadding,
    )
}

/// Currency to precision - formats currency amount
pub fn currency_to_precision(amount: Decimal, precision: i32) -> String {
    decimal_to_precision(
        amount,
        precision,
        RoundingMode::TowardsZero,
        PrecisionMode::DecimalPlaces,
        PaddingMode::NoPadding,
    )
}

/// Count decimal places in a number string
pub fn count_decimal_places(value: &str) -> i32 {
    if let Some(pos) = value.find('.') {
        let decimal_part = &value[pos + 1..];
        decimal_part.trim_end_matches('0').len() as i32
    } else {
        0
    }
}

/// Get precision from a step/tick size value
pub fn precision_from_step(step: Decimal) -> i32 {
    if step.is_zero() {
        return 8; // Default precision
    }

    let step_str = step.to_string();
    count_decimal_places(&step_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_decimal_places() {
        assert_eq!(
            decimal_to_precision(
                dec!(1.23456789),
                4,
                RoundingMode::HalfUp,
                PrecisionMode::DecimalPlaces,
                PaddingMode::NoPadding
            ),
            "1.2346"
        );
    }

    #[test]
    fn test_tick_size() {
        let result = decimal_to_precision_tick(
            dec!(1.23456),
            dec!(0.01),
            RoundingMode::HalfUp,
            PaddingMode::NoPadding,
        );
        assert_eq!(result, "1.23");
    }

    #[test]
    fn test_significant_digits() {
        assert_eq!(
            decimal_to_precision(
                dec!(12345.6789),
                3,
                RoundingMode::HalfUp,
                PrecisionMode::SignificantDigits,
                PaddingMode::NoPadding
            ),
            "12300"
        );
    }

    #[test]
    fn test_amount_to_precision() {
        assert_eq!(amount_to_precision(dec!(1.23456789), 4), "1.2345");
    }

    #[test]
    fn test_price_to_precision() {
        assert_eq!(price_to_precision(dec!(1.23456), 2), "1.23");
    }

    #[test]
    fn test_trailing_zeros() {
        assert_eq!(remove_trailing_zeros("1.23000"), "1.23");
        assert_eq!(remove_trailing_zeros("1.00"), "1");
        assert_eq!(remove_trailing_zeros("123"), "123");
    }
}
