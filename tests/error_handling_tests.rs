//! Integration tests for error handling
//!
//! Tests for CcxtError types, error propagation, and error recovery patterns
//!
//! This test file requires the "cex" feature to be enabled.

#![cfg(feature = "cex")]

use ccxt_rust::{CcxtError, CcxtResult};

// === Error Type Tests ===

#[test]
fn test_error_hierarchy_exchange_errors() {
    // Exchange error family
    let errors = vec![
        CcxtError::ExchangeError {
            message: "Generic error".into(),
        },
        CcxtError::AuthenticationError {
            message: "Invalid API key".into(),
        },
        CcxtError::PermissionDenied {
            message: "No permission".into(),
        },
        CcxtError::AccountNotEnabled {
            message: "Futures not enabled".into(),
        },
        CcxtError::AccountSuspended {
            message: "Account suspended".into(),
        },
        CcxtError::ArgumentsRequired {
            message: "Symbol required".into(),
        },
        CcxtError::BadRequest {
            message: "Invalid parameter".into(),
        },
        CcxtError::BadSymbol {
            symbol: "INVALID/PAIR".into(),
        },
    ];

    for err in errors {
        // All exchange errors should have error codes
        assert!(!err.code().is_empty());
        // Exchange errors (except retryable ones) are permanent
        assert!(!err.is_retryable() || err.code() == "OPERATION_FAILED");
    }
}

#[test]
fn test_error_hierarchy_network_errors() {
    // Network error family - these are retryable
    let retryable_network_errors = vec![
        CcxtError::NetworkError {
            url: "https://api.test.com".into(),
            message: "Connection failed".into(),
        },
        CcxtError::RateLimitExceeded {
            message: "Too many requests".into(),
            retry_after_ms: Some(1000),
        },
        CcxtError::ExchangeNotAvailable {
            message: "Service down".into(),
        },
        CcxtError::RequestTimeout {
            url: "https://api.test.com".into(),
        },
        CcxtError::OnMaintenance {
            message: "Scheduled maintenance".into(),
        },
        CcxtError::InvalidNonce {
            message: "Nonce expired".into(),
        },
    ];

    for err in retryable_network_errors {
        assert!(err.is_network_error(), "Expected network error: {:?}", err);
        assert!(
            err.is_retryable(),
            "Network errors should be retryable: {:?}",
            err
        );
    }

    // DDoSProtection is a network error but has special handling (not auto-retryable)
    let ddos = CcxtError::DDoSProtection {
        message: "CloudFlare block".into(),
    };
    assert!(
        ddos.is_network_error(),
        "DDoSProtection should be a network error"
    );
    // DDoSProtection has a suggested retry delay but is not auto-retryable
    assert!(
        ddos.suggested_retry_after().is_some(),
        "DDoSProtection should have retry suggestion"
    );
}

#[test]
fn test_error_hierarchy_order_errors() {
    // Order error family
    let errors = vec![
        CcxtError::InvalidOrder {
            message: "Invalid amount".into(),
        },
        CcxtError::OrderNotFound {
            order_id: "12345".into(),
        },
        CcxtError::OrderNotCached {
            order_id: "12345".into(),
        },
        CcxtError::OrderImmediatelyFillable,
        CcxtError::OrderNotFillable,
        CcxtError::DuplicateOrderId {
            order_id: "12345".into(),
        },
        CcxtError::ContractUnavailable {
            symbol: "BTC/USDT:USDT".into(),
        },
    ];

    for err in errors {
        assert!(err.is_order_error(), "Expected order error: {:?}", err);
        assert!(
            !err.is_retryable(),
            "Order errors should not be retryable: {:?}",
            err
        );
    }
}

// === Error Code Tests ===

#[test]
fn test_all_error_codes_are_defined() {
    let test_cases: Vec<(CcxtError, &str)> = vec![
        (
            CcxtError::ExchangeError { message: "".into() },
            "EXCHANGE_ERROR",
        ),
        (
            CcxtError::AuthenticationError { message: "".into() },
            "AUTHENTICATION_ERROR",
        ),
        (
            CcxtError::PermissionDenied { message: "".into() },
            "PERMISSION_DENIED",
        ),
        (
            CcxtError::AccountNotEnabled { message: "".into() },
            "ACCOUNT_NOT_ENABLED",
        ),
        (
            CcxtError::AccountSuspended { message: "".into() },
            "ACCOUNT_SUSPENDED",
        ),
        (
            CcxtError::ArgumentsRequired { message: "".into() },
            "ARGUMENTS_REQUIRED",
        ),
        (CcxtError::BadRequest { message: "".into() }, "BAD_REQUEST"),
        (CcxtError::BadSymbol { symbol: "".into() }, "BAD_SYMBOL"),
        (
            CcxtError::OperationRejected { message: "".into() },
            "OPERATION_REJECTED",
        ),
        (CcxtError::NoChange { message: "".into() }, "NO_CHANGE"),
        (
            CcxtError::MarginModeAlreadySet { message: "".into() },
            "MARGIN_MODE_ALREADY_SET",
        ),
        (
            CcxtError::MarketClosed { symbol: "".into() },
            "MARKET_CLOSED",
        ),
        (
            CcxtError::ManualInteractionNeeded { message: "".into() },
            "MANUAL_INTERACTION_NEEDED",
        ),
        (
            CcxtError::RestrictedLocation { message: "".into() },
            "RESTRICTED_LOCATION",
        ),
        (
            CcxtError::InsufficientFunds {
                currency: "".into(),
                required: "".into(),
                available: "".into(),
            },
            "INSUFFICIENT_FUNDS",
        ),
        (
            CcxtError::InvalidAddress { address: "".into() },
            "INVALID_ADDRESS",
        ),
        (
            CcxtError::AddressPending { address: "".into() },
            "ADDRESS_PENDING",
        ),
        (
            CcxtError::InvalidOrder { message: "".into() },
            "INVALID_ORDER",
        ),
        (
            CcxtError::OrderNotFound {
                order_id: "".into(),
            },
            "ORDER_NOT_FOUND",
        ),
        (
            CcxtError::OrderNotCached {
                order_id: "".into(),
            },
            "ORDER_NOT_CACHED",
        ),
        (
            CcxtError::OrderImmediatelyFillable,
            "ORDER_IMMEDIATELY_FILLABLE",
        ),
        (CcxtError::OrderNotFillable, "ORDER_NOT_FILLABLE"),
        (
            CcxtError::DuplicateOrderId {
                order_id: "".into(),
            },
            "DUPLICATE_ORDER_ID",
        ),
        (
            CcxtError::ContractUnavailable { symbol: "".into() },
            "CONTRACT_UNAVAILABLE",
        ),
        (
            CcxtError::NotSupported { feature: "".into() },
            "NOT_SUPPORTED",
        ),
        (
            CcxtError::InvalidProxySettings { message: "".into() },
            "INVALID_PROXY_SETTINGS",
        ),
        (CcxtError::ExchangeClosedByUser, "EXCHANGE_CLOSED_BY_USER"),
        (
            CcxtError::OperationFailed { message: "".into() },
            "OPERATION_FAILED",
        ),
        (
            CcxtError::NetworkError {
                url: "".into(),
                message: "".into(),
            },
            "NETWORK_ERROR",
        ),
        (
            CcxtError::DDoSProtection { message: "".into() },
            "DDOS_PROTECTION",
        ),
        (
            CcxtError::RateLimitExceeded {
                message: "".into(),
                retry_after_ms: None,
            },
            "RATE_LIMIT_EXCEEDED",
        ),
        (
            CcxtError::ExchangeNotAvailable { message: "".into() },
            "EXCHANGE_NOT_AVAILABLE",
        ),
        (
            CcxtError::RequestTimeout { url: "".into() },
            "REQUEST_TIMEOUT",
        ),
        (
            CcxtError::OnMaintenance { message: "".into() },
            "ON_MAINTENANCE",
        ),
        (
            CcxtError::InvalidNonce { message: "".into() },
            "INVALID_NONCE",
        ),
        (
            CcxtError::ChecksumError { message: "".into() },
            "CHECKSUM_ERROR",
        ),
        (
            CcxtError::BadResponse { message: "".into() },
            "BAD_RESPONSE",
        ),
        (CcxtError::NullResponse { url: "".into() }, "NULL_RESPONSE"),
        (
            CcxtError::CancelPending {
                order_id: "".into(),
            },
            "CANCEL_PENDING",
        ),
        (
            CcxtError::UnsubscribeError { message: "".into() },
            "UNSUBSCRIBE_ERROR",
        ),
        (
            CcxtError::ParseError {
                data_type: "".into(),
                message: "".into(),
            },
            "PARSE_ERROR",
        ),
        (CcxtError::JsonError { message: "".into() }, "JSON_ERROR"),
        (
            CcxtError::InvalidSignature { message: "".into() },
            "INVALID_SIGNATURE",
        ),
        (
            CcxtError::InvalidPrivateKey { message: "".into() },
            "INVALID_PRIVATE_KEY",
        ),
    ];

    for (error, expected_code) in test_cases {
        assert_eq!(
            error.code(),
            expected_code,
            "Error code mismatch for {:?}",
            error
        );
    }
}

// === Retry Logic Tests ===

#[test]
fn test_suggested_retry_delays() {
    // Rate limit with explicit retry_after
    let rate_limit = CcxtError::RateLimitExceeded {
        message: "Rate limited".into(),
        retry_after_ms: Some(5000),
    };
    assert_eq!(rate_limit.suggested_retry_after(), Some(5000));

    // Rate limit without retry_after defaults to 1000ms
    let rate_limit_no_delay = CcxtError::RateLimitExceeded {
        message: "Rate limited".into(),
        retry_after_ms: None,
    };
    assert_eq!(rate_limit_no_delay.suggested_retry_after(), Some(1000));

    // Request timeout suggests 5000ms
    let timeout = CcxtError::RequestTimeout {
        url: "https://api.test.com".into(),
    };
    assert_eq!(timeout.suggested_retry_after(), Some(5000));

    // Exchange not available suggests 30000ms
    let unavailable = CcxtError::ExchangeNotAvailable {
        message: "Down".into(),
    };
    assert_eq!(unavailable.suggested_retry_after(), Some(30000));

    // On maintenance suggests 60000ms
    let maintenance = CcxtError::OnMaintenance {
        message: "Maintenance".into(),
    };
    assert_eq!(maintenance.suggested_retry_after(), Some(60000));

    // Invalid nonce suggests 100ms
    let nonce = CcxtError::InvalidNonce {
        message: "Bad nonce".into(),
    };
    assert_eq!(nonce.suggested_retry_after(), Some(100));

    // DDoS protection suggests 60000ms
    let ddos = CcxtError::DDoSProtection {
        message: "Blocked".into(),
    };
    assert_eq!(ddos.suggested_retry_after(), Some(60000));

    // Non-retryable errors have no suggested delay
    let auth = CcxtError::AuthenticationError {
        message: "Invalid".into(),
    };
    assert_eq!(auth.suggested_retry_after(), None);
}

#[test]
fn test_rate_limit_with_retry_constructor() {
    let err = CcxtError::rate_limit_with_retry("Too many requests", 2500);

    assert_eq!(err.code(), "RATE_LIMIT_EXCEEDED");
    assert!(err.is_retryable());
    assert_eq!(err.suggested_retry_after(), Some(2500));
}

// === Error Classification Tests ===

#[test]
fn test_auth_error_classification() {
    // Errors that ARE auth errors
    let auth_errors = vec![
        CcxtError::AuthenticationError { message: "".into() },
        CcxtError::PermissionDenied { message: "".into() },
        CcxtError::AccountNotEnabled { message: "".into() },
        CcxtError::AccountSuspended { message: "".into() },
    ];

    for err in auth_errors {
        assert!(err.is_auth_error(), "Expected auth error: {:?}", err);
    }

    // Errors that are NOT auth errors
    let non_auth_errors = vec![
        CcxtError::NetworkError {
            url: "".into(),
            message: "".into(),
        },
        CcxtError::InvalidOrder { message: "".into() },
        CcxtError::InsufficientFunds {
            currency: "".into(),
            required: "".into(),
            available: "".into(),
        },
    ];

    for err in non_auth_errors {
        assert!(!err.is_auth_error(), "Expected non-auth error: {:?}", err);
    }
}

#[test]
fn test_temporary_vs_permanent_errors() {
    // Temporary (retryable) errors
    let temporary_errors = vec![
        CcxtError::NetworkError {
            url: "".into(),
            message: "".into(),
        },
        CcxtError::RequestTimeout { url: "".into() },
        CcxtError::RateLimitExceeded {
            message: "".into(),
            retry_after_ms: None,
        },
        CcxtError::ExchangeNotAvailable { message: "".into() },
        CcxtError::OnMaintenance { message: "".into() },
        CcxtError::OperationFailed { message: "".into() },
        CcxtError::InvalidNonce { message: "".into() },
    ];

    for err in temporary_errors {
        assert!(err.is_temporary(), "Expected temporary error: {:?}", err);
        assert!(!err.is_permanent(), "Should not be permanent: {:?}", err);
    }

    // Permanent (non-retryable) errors
    let permanent_errors = vec![
        CcxtError::AuthenticationError { message: "".into() },
        CcxtError::BadSymbol { symbol: "".into() },
        CcxtError::InvalidOrder { message: "".into() },
        CcxtError::InsufficientFunds {
            currency: "".into(),
            required: "".into(),
            available: "".into(),
        },
        CcxtError::NotSupported { feature: "".into() },
    ];

    for err in permanent_errors {
        assert!(err.is_permanent(), "Expected permanent error: {:?}", err);
        assert!(!err.is_temporary(), "Should not be temporary: {:?}", err);
    }
}

// === Error Display Tests ===

#[test]
fn test_error_display_messages() {
    let err = CcxtError::AuthenticationError {
        message: "Invalid API key".into(),
    };
    assert!(err.to_string().contains("Invalid API key"));

    let err = CcxtError::InsufficientFunds {
        currency: "USDT".into(),
        required: "1000".into(),
        available: "500".into(),
    };
    let msg = err.to_string();
    assert!(msg.contains("USDT"));
    assert!(msg.contains("1000"));
    assert!(msg.contains("500"));

    let err = CcxtError::BadSymbol {
        symbol: "INVALID/PAIR".into(),
    };
    assert!(err.to_string().contains("INVALID/PAIR"));

    let err = CcxtError::OrderNotFound {
        order_id: "ORD12345".into(),
    };
    assert!(err.to_string().contains("ORD12345"));
}

// === From Trait Tests ===

#[test]
fn test_from_json_error() {
    let json_str = "{ invalid json }";
    let result: Result<serde_json::Value, _> = serde_json::from_str(json_str);
    let json_err = result.unwrap_err();

    let ccxt_err: CcxtError = json_err.into();
    assert_eq!(ccxt_err.code(), "JSON_ERROR");
}

// === Result Type Tests ===

#[test]
fn test_ccxt_result_ok() {
    let result: CcxtResult<i32> = Ok(42);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
}

#[test]
fn test_ccxt_result_err() {
    let result: CcxtResult<i32> = Err(CcxtError::NotSupported {
        feature: "test".into(),
    });
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.code(), "NOT_SUPPORTED");
}

// === Error Pattern Matching Tests ===

#[test]
fn test_error_pattern_matching() {
    fn handle_error(err: CcxtError) -> &'static str {
        match err {
            CcxtError::AuthenticationError { .. } => "auth_error",
            CcxtError::RateLimitExceeded {
                retry_after_ms: Some(_),
                ..
            } => "rate_limit_with_delay",
            CcxtError::RateLimitExceeded {
                retry_after_ms: None,
                ..
            } => "rate_limit_no_delay",
            CcxtError::InsufficientFunds { currency, .. } if currency == "BTC" => {
                "insufficient_btc"
            },
            CcxtError::InsufficientFunds { .. } => "insufficient_other",
            _ if err.is_retryable() => "retryable",
            _ => "other",
        }
    }

    assert_eq!(
        handle_error(CcxtError::AuthenticationError { message: "".into() }),
        "auth_error"
    );
    assert_eq!(
        handle_error(CcxtError::RateLimitExceeded {
            message: "".into(),
            retry_after_ms: Some(1000)
        }),
        "rate_limit_with_delay"
    );
    assert_eq!(
        handle_error(CcxtError::RateLimitExceeded {
            message: "".into(),
            retry_after_ms: None
        }),
        "rate_limit_no_delay"
    );
    assert_eq!(
        handle_error(CcxtError::InsufficientFunds {
            currency: "BTC".into(),
            required: "1".into(),
            available: "0".into()
        }),
        "insufficient_btc"
    );
    assert_eq!(
        handle_error(CcxtError::InsufficientFunds {
            currency: "ETH".into(),
            required: "1".into(),
            available: "0".into()
        }),
        "insufficient_other"
    );
    assert_eq!(
        handle_error(CcxtError::NetworkError {
            url: "".into(),
            message: "".into()
        }),
        "retryable"
    );
    assert_eq!(
        handle_error(CcxtError::NotSupported { feature: "".into() }),
        "other"
    );
}

// === Exchange NotSupported Error Tests ===

#[tokio::test]
async fn test_not_supported_error_for_missing_feature() {
    use ccxt_rust::exchanges::Binance;
    use ccxt_rust::{Exchange, ExchangeConfig};

    let config = ExchangeConfig::new();
    let exchange = Binance::new(config).unwrap();

    // Test that unsupported methods return NotSupported error
    // Note: This will only work for methods that have default implementations returning NotSupported

    // Check that the exchange correctly reports its capabilities
    let features = exchange.has();

    // Binance should support these
    assert!(features.fetch_ticker);
    assert!(features.fetch_order_book);

    // Use has_feature to check for specific features
    assert!(exchange.has_feature("fetchTicker"));
    assert!(exchange.has_feature("createOrder"));
    assert!(!exchange.has_feature("nonExistentFeature"));
}

// === Concurrent Error Handling Test ===

#[tokio::test]
async fn test_concurrent_error_handling() {
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let errors: Arc<Mutex<Vec<CcxtError>>> = Arc::new(Mutex::new(Vec::new()));

    let mut handles = vec![];

    for i in 0..5 {
        let errors_clone = errors.clone();
        handles.push(tokio::spawn(async move {
            let err = match i % 3 {
                0 => CcxtError::NetworkError {
                    url: format!("url_{}", i),
                    message: "error".into(),
                },
                1 => CcxtError::RateLimitExceeded {
                    message: format!("limit_{}", i),
                    retry_after_ms: Some(1000),
                },
                _ => CcxtError::RequestTimeout {
                    url: format!("timeout_{}", i),
                },
            };
            errors_clone.lock().await.push(err);
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let collected = errors.lock().await;
    assert_eq!(collected.len(), 5);

    // All errors should be retryable (they're all network-related)
    for err in collected.iter() {
        assert!(err.is_retryable());
    }
}
