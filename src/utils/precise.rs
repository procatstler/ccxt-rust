//! Precise - High-precision arithmetic for cryptocurrency calculations
//!
//! This module provides arbitrary precision arithmetic using BigInt,
//! designed for accurate financial calculations in cryptocurrency trading.

use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// Base for decimal calculations (10)
fn base() -> BigInt {
    BigInt::from(10)
}

/// Precise number with arbitrary precision decimal support
#[derive(Clone, Debug)]
pub struct Precise {
    /// The integer representation (scaled by 10^decimals)
    pub integer: BigInt,
    /// Number of decimal places
    pub decimals: i32,
}

impl Precise {
    /// Create a new Precise from BigInt and decimal count
    pub fn new(integer: BigInt, decimals: i32) -> Self {
        Precise { integer, decimals }
    }

    /// Create a new Precise from a string representation
    pub fn from_string(number: &str) -> Self {
        let number = number.to_lowercase();

        // Handle scientific notation
        let (num_part, modifier) = if let Some(e_pos) = number.find('e') {
            let (num, exp) = number.split_at(e_pos);
            let modifier: i32 = exp[1..].parse().unwrap_or(0);
            (num.to_string(), modifier)
        } else {
            (number, 0)
        };

        // Find decimal point
        let decimal_index = num_part.find('.');
        let decimals = if let Some(idx) = decimal_index {
            (num_part.len() - idx - 1) as i32
        } else {
            0
        };

        // Remove decimal point and parse as integer
        let integer_string = num_part.replace('.', "");
        let integer = BigInt::from_str(&integer_string).unwrap_or_else(|_| BigInt::zero());

        Precise {
            integer,
            decimals: decimals - modifier,
        }
    }

    /// Multiply two Precise numbers
    pub fn mul(&self, other: &Precise) -> Precise {
        let integer_result = &self.integer * &other.integer;
        Precise::new(integer_result, self.decimals + other.decimals)
    }

    /// Divide two Precise numbers with specified precision
    pub fn div(&self, other: &Precise, precision: i32) -> Precise {
        let distance = precision - self.decimals + other.decimals;

        let numerator = if distance == 0 {
            self.integer.clone()
        } else if distance < 0 {
            let exponent = base().pow((-distance) as u32);
            &self.integer / &exponent
        } else {
            let exponent = base().pow(distance as u32);
            &self.integer * &exponent
        };

        let result = &numerator / &other.integer;
        Precise::new(result, precision)
    }

    /// Add two Precise numbers
    pub fn add(&self, other: &Precise) -> Precise {
        if self.decimals == other.decimals {
            let integer_result = &self.integer + &other.integer;
            Precise::new(integer_result, self.decimals)
        } else {
            let (smaller, bigger) = if self.decimals > other.decimals {
                (other, self)
            } else {
                (self, other)
            };

            let exponent = (bigger.decimals - smaller.decimals) as u32;
            let normalised = &smaller.integer * base().pow(exponent);
            let result = &normalised + &bigger.integer;
            Precise::new(result, bigger.decimals)
        }
    }

    /// Subtract two Precise numbers
    pub fn sub(&self, other: &Precise) -> Precise {
        let negative = Precise::new(-&other.integer, other.decimals);
        self.add(&negative)
    }

    /// Modulo operation
    pub fn modulo(&self, other: &Precise) -> Precise {
        let rationizer_numerator = std::cmp::max(-self.decimals + other.decimals, 0) as u32;
        let numerator = &self.integer * base().pow(rationizer_numerator);

        let rationizer_denominator = std::cmp::max(-other.decimals + self.decimals, 0) as u32;
        let denominator = &other.integer * base().pow(rationizer_denominator);

        let result = &numerator % &denominator;
        Precise::new(result, rationizer_denominator as i32 + other.decimals)
    }

    /// Absolute value
    pub fn abs(&self) -> Precise {
        Precise::new(self.integer.abs(), self.decimals)
    }

    /// Negate the number
    pub fn neg(&self) -> Precise {
        Precise::new(-&self.integer, self.decimals)
    }

    /// Bitwise OR
    pub fn or(&self, other: &Precise) -> Precise {
        let integer_result = &self.integer | &other.integer;
        Precise::new(integer_result, self.decimals)
    }

    /// Returns the minimum of two Precise numbers
    pub fn precise_min(&self, other: &Precise) -> Precise {
        if self.lt(other) {
            self.clone()
        } else {
            other.clone()
        }
    }

    /// Returns the maximum of two Precise numbers
    pub fn precise_max(&self, other: &Precise) -> Precise {
        if self.gt(other) {
            self.clone()
        } else {
            other.clone()
        }
    }

    /// Greater than comparison
    pub fn gt(&self, other: &Precise) -> bool {
        let sum = self.sub(other);
        sum.integer > BigInt::zero()
    }

    /// Greater than or equal comparison
    pub fn ge(&self, other: &Precise) -> bool {
        let sum = self.sub(other);
        sum.integer >= BigInt::zero()
    }

    /// Less than comparison
    pub fn lt(&self, other: &Precise) -> bool {
        other.gt(self)
    }

    /// Less than or equal comparison
    pub fn le(&self, other: &Precise) -> bool {
        other.ge(self)
    }

    /// Reduce the number by removing trailing zeros
    pub fn reduce(&mut self) {
        let string = self.integer.to_string();
        let string = string.trim_start_matches('-');

        if string == "0" {
            self.decimals = 0;
            return;
        }

        let start = string.len();
        if start == 0 {
            return;
        }

        let mut trailing_zeros = 0;
        for c in string.chars().rev() {
            if c == '0' {
                trailing_zeros += 1;
            } else {
                break;
            }
        }

        if trailing_zeros > 0 {
            self.decimals -= trailing_zeros as i32;
            let new_len = string.len() - trailing_zeros;
            let is_negative = self.integer < BigInt::zero();
            let new_string = &string[..new_len];
            self.integer = BigInt::from_str(new_string).unwrap_or_else(|_| BigInt::zero());
            if is_negative {
                self.integer = -&self.integer;
            }
        }
    }

    /// Check equality with another Precise
    pub fn equals(&self, other: &Precise) -> bool {
        let mut a = self.clone();
        let mut b = other.clone();
        a.reduce();
        b.reduce();
        a.decimals == b.decimals && a.integer == b.integer
    }

    /// Convert to string representation
    pub fn to_str_repr(&self) -> String {
        let mut copy = self.clone();
        copy.reduce();

        let (sign, abs) = if copy.integer < BigInt::zero() {
            ("-", -&copy.integer)
        } else {
            ("", copy.integer.clone())
        };

        let abs_string = abs.to_string();

        if copy.decimals <= 0 {
            // No decimal point needed, may need trailing zeros
            if copy.decimals < 0 {
                return format!("{}{}{}", sign, abs_string, "0".repeat((-copy.decimals) as usize));
            }
            return format!("{sign}{abs_string}");
        }

        // Need to add decimal point
        let padded = format!("{:0>width$}", abs_string, width = copy.decimals as usize);
        let decimal_pos = padded.len() as i32 - copy.decimals;

        if decimal_pos <= 0 {
            // All digits are after decimal point
            let zeros_needed = (-decimal_pos) as usize;
            return format!("{}0.{}{}", sign, "0".repeat(zeros_needed), padded);
        }

        let (integer_part, decimal_part) = padded.split_at(decimal_pos as usize);
        if decimal_part.is_empty() {
            format!("{sign}{integer_part}")
        } else {
            format!("{sign}{integer_part}.{decimal_part}")
        }
    }

    // Static string operations that return Option<String>

    /// Multiply two string numbers
    pub fn string_mul(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).mul(&Precise::from_string(s2)).to_str_repr())
    }

    /// Divide two string numbers with optional precision
    pub fn string_div(string1: Option<&str>, string2: Option<&str>, precision: Option<i32>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        let p2 = Precise::from_string(s2);
        if p2.integer.is_zero() {
            return None;
        }
        Some(Precise::from_string(s1).div(&p2, precision.unwrap_or(18)).to_str_repr())
    }

    /// Add two string numbers
    pub fn string_add(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        match (string1, string2) {
            (None, None) => None,
            (Some(s), None) => Some(s.to_string()),
            (None, Some(s)) => Some(s.to_string()),
            (Some(s1), Some(s2)) => {
                Some(Precise::from_string(s1).add(&Precise::from_string(s2)).to_str_repr())
            }
        }
    }

    /// Subtract two string numbers
    pub fn string_sub(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).sub(&Precise::from_string(s2)).to_str_repr())
    }

    /// Absolute value of string number
    pub fn string_abs(string: Option<&str>) -> Option<String> {
        let s = string?;
        Some(Precise::from_string(s).abs().to_str_repr())
    }

    /// Negate string number
    pub fn string_neg(string: Option<&str>) -> Option<String> {
        let s = string?;
        Some(Precise::from_string(s).neg().to_str_repr())
    }

    /// Modulo of two string numbers
    pub fn string_mod(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).modulo(&Precise::from_string(s2)).to_str_repr())
    }

    /// Bitwise OR of two string numbers
    pub fn string_or(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).or(&Precise::from_string(s2)).to_str_repr())
    }

    /// Check equality of two string numbers
    pub fn string_equals(string1: Option<&str>, string2: Option<&str>) -> Option<bool> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).equals(&Precise::from_string(s2)))
    }

    /// Alias for string_equals
    pub fn string_eq(string1: Option<&str>, string2: Option<&str>) -> Option<bool> {
        Precise::string_equals(string1, string2)
    }

    /// Minimum of two string numbers
    pub fn string_min(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).precise_min(&Precise::from_string(s2)).to_str_repr())
    }

    /// Maximum of two string numbers
    pub fn string_max(string1: Option<&str>, string2: Option<&str>) -> Option<String> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).precise_max(&Precise::from_string(s2)).to_str_repr())
    }

    /// Greater than comparison of two string numbers
    pub fn string_gt(string1: Option<&str>, string2: Option<&str>) -> Option<bool> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).gt(&Precise::from_string(s2)))
    }

    /// Greater than or equal comparison of two string numbers
    pub fn string_ge(string1: Option<&str>, string2: Option<&str>) -> Option<bool> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).ge(&Precise::from_string(s2)))
    }

    /// Less than comparison of two string numbers
    pub fn string_lt(string1: Option<&str>, string2: Option<&str>) -> Option<bool> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).lt(&Precise::from_string(s2)))
    }

    /// Less than or equal comparison of two string numbers
    pub fn string_le(string1: Option<&str>, string2: Option<&str>) -> Option<bool> {
        let (s1, s2) = (string1?, string2?);
        Some(Precise::from_string(s1).le(&Precise::from_string(s2)))
    }
}

impl fmt::Display for Precise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_str_repr())
    }
}

impl PartialEq for Precise {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl Eq for Precise {}

impl PartialOrd for Precise {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Precise {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.gt(other) {
            Ordering::Greater
        } else if self.lt(other) {
            Ordering::Less
        } else {
            Ordering::Equal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_string_integer() {
        let p = Precise::from_string("123");
        assert_eq!(p.integer, BigInt::from(123));
        assert_eq!(p.decimals, 0);
    }

    #[test]
    fn test_from_string_decimal() {
        let p = Precise::from_string("123.456");
        assert_eq!(p.integer, BigInt::from(123456));
        assert_eq!(p.decimals, 3);
    }

    #[test]
    fn test_from_string_scientific() {
        let p = Precise::from_string("1.5e2");
        assert_eq!(p.to_string(), "150");
    }

    #[test]
    fn test_mul() {
        let a = Precise::from_string("2.5");
        let b = Precise::from_string("4");
        assert_eq!(a.mul(&b).to_string(), "10");
    }

    #[test]
    fn test_div() {
        let a = Precise::from_string("10");
        let b = Precise::from_string("4");
        assert_eq!(a.div(&b, 2).to_string(), "2.5");
    }

    #[test]
    fn test_add() {
        let a = Precise::from_string("1.5");
        let b = Precise::from_string("2.5");
        assert_eq!(a.add(&b).to_string(), "4");
    }

    #[test]
    fn test_sub() {
        let a = Precise::from_string("5.5");
        let b = Precise::from_string("2.5");
        assert_eq!(a.sub(&b).to_string(), "3");
    }

    #[test]
    fn test_abs() {
        let a = Precise::from_string("-5.5");
        assert_eq!(a.abs().to_string(), "5.5");
    }

    #[test]
    fn test_neg() {
        let a = Precise::from_string("5.5");
        assert_eq!(a.neg().to_string(), "-5.5");
    }

    #[test]
    fn test_comparisons() {
        let a = Precise::from_string("5.5");
        let b = Precise::from_string("3.3");

        assert!(a.gt(&b));
        assert!(a.ge(&b));
        assert!(!a.lt(&b));
        assert!(!a.le(&b));

        let c = Precise::from_string("5.5");
        assert!(a.ge(&c));
        assert!(a.le(&c));
        assert!(a.equals(&c));
    }

    #[test]
    fn test_min_max() {
        let a = Precise::from_string("5.5");
        let b = Precise::from_string("3.3");

        assert_eq!(a.precise_min(&b).to_string(), "3.3");
        assert_eq!(a.precise_max(&b).to_string(), "5.5");
    }

    #[test]
    fn test_string_operations() {
        assert_eq!(
            Precise::string_mul(Some("2.5"), Some("4")),
            Some("10".to_string())
        );
        assert_eq!(
            Precise::string_add(Some("1.5"), Some("2.5")),
            Some("4".to_string())
        );
        assert_eq!(
            Precise::string_sub(Some("5"), Some("2")),
            Some("3".to_string())
        );
        assert_eq!(
            Precise::string_div(Some("10"), Some("4"), Some(2)),
            Some("2.5".to_string())
        );
    }

    #[test]
    fn test_string_operations_with_none() {
        assert_eq!(Precise::string_mul(None, Some("4")), None);
        assert_eq!(Precise::string_mul(Some("2"), None), None);
        assert_eq!(Precise::string_add(None, None), None);
        assert_eq!(Precise::string_add(Some("1"), None), Some("1".to_string()));
        assert_eq!(Precise::string_add(None, Some("2")), Some("2".to_string()));
    }

    #[test]
    fn test_div_by_zero() {
        assert_eq!(Precise::string_div(Some("10"), Some("0"), None), None);
    }

    #[test]
    fn test_high_precision() {
        let a = Precise::from_string("0.123456789012345678");
        let b = Precise::from_string("0.000000000000000001");
        let result = a.add(&b);
        assert_eq!(result.to_string(), "0.123456789012345679");
    }

    #[test]
    fn test_large_numbers() {
        let a = Precise::from_string("999999999999999999999999999999");
        let b = Precise::from_string("1");
        let result = a.add(&b);
        assert_eq!(result.to_string(), "1000000000000000000000000000000");
    }
}
