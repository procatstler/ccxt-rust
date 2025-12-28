//! Safe helper functions for extracting values from JSON
//!
//! CCXT의 safe* 헬퍼 함수들을 Rust로 구현

use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

/// 안전한 문자열 추출
pub fn safe_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

/// 두 키 중 하나에서 문자열 추출
pub fn safe_string2(obj: &Value, key1: &str, key2: &str) -> Option<String> {
    safe_string(obj, key1).or_else(|| safe_string(obj, key2))
}

/// N개 키 중 하나에서 문자열 추출
pub fn safe_string_n(obj: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| safe_string(obj, k))
}

/// 소문자 문자열 추출
pub fn safe_string_lower(obj: &Value, key: &str) -> Option<String> {
    safe_string(obj, key).map(|s| s.to_lowercase())
}

/// 대문자 문자열 추출
pub fn safe_string_upper(obj: &Value, key: &str) -> Option<String> {
    safe_string(obj, key).map(|s| s.to_uppercase())
}

/// 안전한 정수 추출
pub fn safe_integer(obj: &Value, key: &str) -> Option<i64> {
    obj.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// 두 키 중 하나에서 정수 추출
pub fn safe_integer2(obj: &Value, key1: &str, key2: &str) -> Option<i64> {
    safe_integer(obj, key1).or_else(|| safe_integer(obj, key2))
}

/// N개 키 중 하나에서 정수 추출
pub fn safe_integer_n(obj: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| safe_integer(obj, k))
}

/// 정수 * 배수
pub fn safe_integer_product(obj: &Value, key: &str, factor: i64) -> Option<i64> {
    safe_integer(obj, key).map(|v| v * factor)
}

/// 안전한 실수 추출
pub fn safe_float(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// 두 키 중 하나에서 실수 추출
pub fn safe_float2(obj: &Value, key1: &str, key2: &str) -> Option<f64> {
    safe_float(obj, key1).or_else(|| safe_float(obj, key2))
}

/// N개 키 중 하나에서 실수 추출
pub fn safe_float_n(obj: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| safe_float(obj, k))
}

/// 안전한 Decimal 추출
pub fn safe_decimal(obj: &Value, key: &str) -> Option<Decimal> {
    obj.get(key).and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Decimal::from_str(s).ok(),
        Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    })
}

/// 두 키 중 하나에서 Decimal 추출
pub fn safe_decimal2(obj: &Value, key1: &str, key2: &str) -> Option<Decimal> {
    safe_decimal(obj, key1).or_else(|| safe_decimal(obj, key2))
}

/// N개 키 중 하나에서 Decimal 추출
pub fn safe_decimal_n(obj: &Value, keys: &[&str]) -> Option<Decimal> {
    keys.iter().find_map(|k| safe_decimal(obj, k))
}

/// 안전한 타임스탬프 추출 (밀리초)
pub fn safe_timestamp(obj: &Value, key: &str) -> Option<i64> {
    safe_integer(obj, key).or_else(|| {
        safe_string(obj, key).and_then(|s| {
            // ISO 8601 파싱 시도
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.timestamp_millis())
                .ok()
        })
    })
}

/// 두 키 중 하나에서 타임스탬프 추출
pub fn safe_timestamp2(obj: &Value, key1: &str, key2: &str) -> Option<i64> {
    safe_timestamp(obj, key1).or_else(|| safe_timestamp(obj, key2))
}

/// N개 키 중 하나에서 타임스탬프 추출
pub fn safe_timestamp_n(obj: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| safe_timestamp(obj, k))
}

/// 안전한 값 추출
pub fn safe_value<'a>(obj: &'a Value, key: &str) -> Option<&'a Value> {
    obj.get(key).filter(|v| !v.is_null())
}

/// 두 키 중 하나에서 값 추출
pub fn safe_value2<'a>(obj: &'a Value, key1: &str, key2: &str) -> Option<&'a Value> {
    safe_value(obj, key1).or_else(|| safe_value(obj, key2))
}

/// N개 키 중 하나에서 값 추출
pub fn safe_value_n<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| safe_value(obj, k))
}

/// 안전한 불린 추출
pub fn safe_bool(obj: &Value, key: &str) -> Option<bool> {
    obj.get(key).and_then(|v| match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.to_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        _ => None,
    })
}

/// 두 키 중 하나에서 불린 추출
pub fn safe_bool2(obj: &Value, key1: &str, key2: &str) -> Option<bool> {
    safe_bool(obj, key1).or_else(|| safe_bool(obj, key2))
}

/// 기본값을 가진 안전한 문자열 추출
pub fn safe_string_or(obj: &Value, key: &str, default: &str) -> String {
    safe_string(obj, key).unwrap_or_else(|| default.to_string())
}

/// 기본값을 가진 안전한 정수 추출
pub fn safe_integer_or(obj: &Value, key: &str, default: i64) -> i64 {
    safe_integer(obj, key).unwrap_or(default)
}

/// 기본값을 가진 안전한 실수 추출
pub fn safe_float_or(obj: &Value, key: &str, default: f64) -> f64 {
    safe_float(obj, key).unwrap_or(default)
}

/// 기본값을 가진 안전한 Decimal 추출
pub fn safe_decimal_or(obj: &Value, key: &str, default: Decimal) -> Decimal {
    safe_decimal(obj, key).unwrap_or(default)
}

/// 기본값을 가진 안전한 불린 추출
pub fn safe_bool_or(obj: &Value, key: &str, default: bool) -> bool {
    safe_bool(obj, key).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    // === safe_string tests ===

    #[test]
    fn test_safe_string_from_string() {
        let obj = json!({"name": "BTC/USDT"});
        assert_eq!(safe_string(&obj, "name"), Some("BTC/USDT".to_string()));
    }

    #[test]
    fn test_safe_string_from_number() {
        let obj = json!({"id": 12345});
        assert_eq!(safe_string(&obj, "id"), Some("12345".to_string()));
    }

    #[test]
    fn test_safe_string_from_bool() {
        let obj = json!({"active": true});
        assert_eq!(safe_string(&obj, "active"), Some("true".to_string()));
    }

    #[test]
    fn test_safe_string_missing_key() {
        let obj = json!({"name": "BTC"});
        assert_eq!(safe_string(&obj, "missing"), None);
    }

    #[test]
    fn test_safe_string_null_value() {
        let obj = json!({"name": null});
        assert_eq!(safe_string(&obj, "name"), None);
    }

    #[test]
    fn test_safe_string2() {
        let obj = json!({"symbol": "BTCUSDT"});
        assert_eq!(safe_string2(&obj, "id", "symbol"), Some("BTCUSDT".to_string()));
        assert_eq!(safe_string2(&obj, "missing1", "missing2"), None);
    }

    #[test]
    fn test_safe_string_n() {
        let obj = json!({"s": "BTC"});
        assert_eq!(safe_string_n(&obj, &["symbol", "sym", "s"]), Some("BTC".to_string()));
        assert_eq!(safe_string_n(&obj, &["missing1", "missing2"]), None);
    }

    #[test]
    fn test_safe_string_lower() {
        let obj = json!({"status": "OPEN"});
        assert_eq!(safe_string_lower(&obj, "status"), Some("open".to_string()));
    }

    #[test]
    fn test_safe_string_upper() {
        let obj = json!({"side": "buy"});
        assert_eq!(safe_string_upper(&obj, "side"), Some("BUY".to_string()));
    }

    // === safe_integer tests ===

    #[test]
    fn test_safe_integer_from_number() {
        let obj = json!({"timestamp": 1699999999999i64});
        assert_eq!(safe_integer(&obj, "timestamp"), Some(1699999999999));
    }

    #[test]
    fn test_safe_integer_from_string() {
        let obj = json!({"count": "100"});
        assert_eq!(safe_integer(&obj, "count"), Some(100));
    }

    #[test]
    fn test_safe_integer_invalid_string() {
        let obj = json!({"count": "not_a_number"});
        assert_eq!(safe_integer(&obj, "count"), None);
    }

    #[test]
    fn test_safe_integer2() {
        let obj = json!({"ts": 1000});
        assert_eq!(safe_integer2(&obj, "timestamp", "ts"), Some(1000));
    }

    #[test]
    fn test_safe_integer_n() {
        let obj = json!({"T": 2000});
        assert_eq!(safe_integer_n(&obj, &["timestamp", "ts", "T"]), Some(2000));
    }

    #[test]
    fn test_safe_integer_product() {
        let obj = json!({"seconds": 5});
        assert_eq!(safe_integer_product(&obj, "seconds", 1000), Some(5000));
    }

    // === safe_float tests ===

    #[test]
    fn test_safe_float_from_number() {
        let obj = json!({"price": 50000.50});
        assert_eq!(safe_float(&obj, "price"), Some(50000.50));
    }

    #[test]
    fn test_safe_float_from_string() {
        let obj = json!({"rate": "0.001"});
        assert_eq!(safe_float(&obj, "rate"), Some(0.001));
    }

    #[test]
    fn test_safe_float2() {
        let obj = json!({"p": 100.5});
        assert_eq!(safe_float2(&obj, "price", "p"), Some(100.5));
    }

    // === safe_decimal tests ===

    #[test]
    fn test_safe_decimal_from_string() {
        let obj = json!({"amount": "1.23456789"});
        assert_eq!(safe_decimal(&obj, "amount"), Some(dec!(1.23456789)));
    }

    #[test]
    fn test_safe_decimal_from_number() {
        let obj = json!({"amount": 1.5});
        assert_eq!(safe_decimal(&obj, "amount"), Some(dec!(1.5)));
    }

    #[test]
    fn test_safe_decimal_empty_string() {
        let obj = json!({"amount": ""});
        assert_eq!(safe_decimal(&obj, "amount"), None);
    }

    #[test]
    fn test_safe_decimal2() {
        let obj = json!({"qty": "10.5"});
        assert_eq!(safe_decimal2(&obj, "amount", "qty"), Some(dec!(10.5)));
    }

    #[test]
    fn test_safe_decimal_n() {
        let obj = json!({"sz": "5.0"});
        assert_eq!(safe_decimal_n(&obj, &["amount", "qty", "sz"]), Some(dec!(5.0)));
    }

    // === safe_timestamp tests ===

    #[test]
    fn test_safe_timestamp_from_integer() {
        let obj = json!({"timestamp": 1699999999999i64});
        assert_eq!(safe_timestamp(&obj, "timestamp"), Some(1699999999999));
    }

    #[test]
    fn test_safe_timestamp_from_iso8601() {
        let obj = json!({"datetime": "2023-11-15T10:30:00+00:00"});
        let result = safe_timestamp(&obj, "datetime");
        assert!(result.is_some());
    }

    #[test]
    fn test_safe_timestamp2() {
        let obj = json!({"ts": 1000});
        assert_eq!(safe_timestamp2(&obj, "timestamp", "ts"), Some(1000));
    }

    // === safe_value tests ===

    #[test]
    fn test_safe_value() {
        let obj = json!({"data": {"nested": true}});
        let result = safe_value(&obj, "data");
        assert!(result.is_some());
        assert_eq!(result.unwrap().get("nested"), Some(&json!(true)));
    }

    #[test]
    fn test_safe_value_null() {
        let obj = json!({"data": null});
        assert_eq!(safe_value(&obj, "data"), None);
    }

    #[test]
    fn test_safe_value2() {
        let obj = json!({"info": {"key": "value"}});
        assert!(safe_value2(&obj, "data", "info").is_some());
    }

    // === safe_bool tests ===

    #[test]
    fn test_safe_bool_from_bool() {
        let obj = json!({"active": true});
        assert_eq!(safe_bool(&obj, "active"), Some(true));
    }

    #[test]
    fn test_safe_bool_from_string() {
        let obj = json!({"enabled": "true"});
        assert_eq!(safe_bool(&obj, "enabled"), Some(true));

        let obj2 = json!({"enabled": "false"});
        assert_eq!(safe_bool(&obj2, "enabled"), Some(false));

        let obj3 = json!({"enabled": "1"});
        assert_eq!(safe_bool(&obj3, "enabled"), Some(true));
    }

    #[test]
    fn test_safe_bool_from_number() {
        let obj = json!({"flag": 1});
        assert_eq!(safe_bool(&obj, "flag"), Some(true));

        let obj2 = json!({"flag": 0});
        assert_eq!(safe_bool(&obj2, "flag"), Some(false));
    }

    // === *_or default value tests ===

    #[test]
    fn test_safe_string_or() {
        let obj = json!({"name": "BTC"});
        assert_eq!(safe_string_or(&obj, "name", "default"), "BTC");
        assert_eq!(safe_string_or(&obj, "missing", "default"), "default");
    }

    #[test]
    fn test_safe_integer_or() {
        let obj = json!({"count": 10});
        assert_eq!(safe_integer_or(&obj, "count", 0), 10);
        assert_eq!(safe_integer_or(&obj, "missing", 0), 0);
    }

    #[test]
    fn test_safe_float_or() {
        let obj = json!({"rate": 0.5});
        assert_eq!(safe_float_or(&obj, "rate", 0.0), 0.5);
        assert_eq!(safe_float_or(&obj, "missing", 1.0), 1.0);
    }

    #[test]
    fn test_safe_decimal_or() {
        let obj = json!({"amount": "10.5"});
        assert_eq!(safe_decimal_or(&obj, "amount", Decimal::ZERO), dec!(10.5));
        assert_eq!(safe_decimal_or(&obj, "missing", dec!(1.0)), dec!(1.0));
    }

    #[test]
    fn test_safe_bool_or() {
        let obj = json!({"active": true});
        assert!(safe_bool_or(&obj, "active", false));
        assert!(!safe_bool_or(&obj, "missing", false));
    }

    // === Edge cases ===

    #[test]
    fn test_nested_object() {
        let obj = json!({
            "data": {
                "price": "100.5",
                "amount": 10
            }
        });

        if let Some(data) = safe_value(&obj, "data") {
            assert_eq!(safe_decimal(data, "price"), Some(dec!(100.5)));
            assert_eq!(safe_integer(data, "amount"), Some(10));
        } else {
            panic!("Expected nested object");
        }
    }

    #[test]
    fn test_array_value() {
        let obj = json!({"items": [1, 2, 3]});
        let value = safe_value(&obj, "items");
        assert!(value.is_some());
        assert!(value.unwrap().is_array());
    }
}
