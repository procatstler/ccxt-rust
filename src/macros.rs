//! Exchange Implementation Macros
//!
//! Provides macros to reduce boilerplate code in exchange implementations.
//!
//! # Available Macros
//!
//! - [`timeframe_map!`] - Create timeframe HashMap with minimal syntax
//! - [`feature_flags!`] - Create ExchangeFeatures with enabled features listed
//! - [`api_urls!`] - Create API URL HashMap
//! - [`exchange_urls!`] - Create ExchangeUrls struct

/// Creates a HashMap<Timeframe, String> with minimal syntax.
///
/// # Example
///
/// ```ignore
/// let timeframes = timeframe_map! {
///     Minute1 => "1m",
///     Minute5 => "5m",
///     Hour1 => "1h",
///     Day1 => "1d",
/// };
/// ```
#[macro_export]
macro_rules! timeframe_map {
    ($($variant:ident => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut map = std::collections::HashMap::new();
        $(
            map.insert($crate::types::Timeframe::$variant, $value.into());
        )*
        map
    }};
}

/// Creates ExchangeFeatures with only enabled features listed.
///
/// All unlisted features default to `false`.
///
/// # Example
///
/// ```ignore
/// let features = feature_flags! {
///     spot,
///     margin,
///     fetch_markets,
///     fetch_ticker,
///     create_order,
///     ws,
///     watch_ticker,
/// };
/// ```
#[macro_export]
macro_rules! feature_flags {
    ($($feature:ident),* $(,)?) => {{
        #[allow(clippy::needless_update)]
        $crate::types::ExchangeFeatures {
            $(
                $feature: true,
            )*
            ..Default::default()
        }
    }};
}

/// Creates an API URLs HashMap for exchange configuration.
///
/// # Example
///
/// ```ignore
/// let api_urls = api_urls! {
///     "public" => "https://api.exchange.com",
///     "private" => "https://api.exchange.com",
/// };
/// ```
#[macro_export]
macro_rules! api_urls {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut map = std::collections::HashMap::new();
        $(
            map.insert($key.into(), $value.into());
        )*
        map
    }};
}

/// Creates ExchangeUrls struct with builder-like syntax.
///
/// # Example
///
/// ```ignore
/// let urls = exchange_urls! {
///     logo: "https://example.com/logo.png",
///     www: "https://www.example.com",
///     api: {
///         "public" => "https://api.example.com",
///         "private" => "https://api.example.com",
///     },
///     doc: [
///         "https://docs.example.com",
///     ],
///     fees: "https://example.com/fees",
/// };
/// ```
#[macro_export]
macro_rules! exchange_urls {
    (
        $(logo: $logo:expr,)?
        $(www: $www:expr,)?
        api: {
            $($api_key:expr => $api_value:expr),* $(,)?
        },
        doc: [$($doc:expr),* $(,)?],
        $(fees: $fees:expr,)?
    ) => {{
        let mut api = std::collections::HashMap::new();
        $(
            api.insert($api_key.into(), $api_value.into());
        )*
        $crate::types::ExchangeUrls {
            logo: None $(.or(Some($logo.into())))?,
            www: None $(.or(Some($www.into())))?,
            api,
            doc: vec![$($doc.into()),*],
            fees: None $(.or(Some($fees.into())))?,
        }
    }};
}

/// Macro for common exchange error mapping.
///
/// Maps exchange-specific error codes to CcxtError variants.
///
/// # Example
///
/// ```ignore
/// map_exchange_error! {
///     response,
///     Binance,
///     // Error code => CcxtError variant
///     -1000 => ExchangeError,
///     -1001 => ExchangeNotAvailable,
///     -1002 => AuthenticationError,
///     -1003 => RateLimitExceeded,
///     -1021 => InvalidNonce,
///     -2010 => InsufficientFunds,
///     -2011 => OrderNotFound,
/// }
/// ```
#[macro_export]
macro_rules! map_exchange_error {
    ($response:expr, $exchange:ident, $($code:expr => $variant:ident),* $(,)?) => {{
        let code = $response.code;
        let msg = $response.msg.clone().unwrap_or_default();
        match code {
            $(
                $code => $crate::errors::CcxtError::$variant {
                    message: format!("[{}] {}: {}", stringify!($exchange), code, msg),
                },
            )*
            _ => $crate::errors::CcxtError::ExchangeError {
                message: format!("[{}] Unknown error {}: {}", stringify!($exchange), code, msg),
            },
        }
    }};
}

/// Helper macro to validate required credentials.
///
/// Returns AuthenticationError if credentials are missing.
///
/// # Example
///
/// ```ignore
/// let (api_key, secret) = require_credentials!(self.config)?;
/// ```
#[macro_export]
macro_rules! require_credentials {
    ($config:expr) => {{
        let api_key = $config
            .api_key()
            .ok_or_else(|| $crate::errors::CcxtError::AuthenticationError {
                message: "API key required".into(),
            })?;
        let secret = $config
            .secret()
            .ok_or_else(|| $crate::errors::CcxtError::AuthenticationError {
                message: "Secret required".into(),
            })?;
        Ok::<_, $crate::errors::CcxtError>((api_key, secret))
    }};
}

/// Macro to build query string from parameters.
///
/// # Example
///
/// ```ignore
/// let query = build_query_string!(params);
/// let query = build_query_string!(params, url_encode: true);
/// ```
#[macro_export]
macro_rules! build_query_string {
    ($params:expr) => {{
        $params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&")
    }};
    ($params:expr, url_encode: true) => {{
        $params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    }};
}

#[cfg(test)]
mod tests {
    use crate::types::{ExchangeFeatures, ExchangeUrls, Timeframe};
    use std::collections::HashMap;

    #[test]
    fn test_timeframe_map() {
        let timeframes: HashMap<Timeframe, String> = timeframe_map! {
            Minute1 => "1m",
            Minute5 => "5m",
            Hour1 => "1h",
            Day1 => "1d",
        };

        assert_eq!(timeframes.get(&Timeframe::Minute1), Some(&"1m".to_string()));
        assert_eq!(timeframes.get(&Timeframe::Hour1), Some(&"1h".to_string()));
        assert_eq!(timeframes.len(), 4);
    }

    #[test]
    fn test_feature_flags() {
        let features: ExchangeFeatures = feature_flags! {
            spot,
            margin,
            fetch_markets,
            fetch_ticker,
            create_order,
            ws,
            watch_ticker,
        };

        assert!(features.spot);
        assert!(features.margin);
        assert!(features.fetch_markets);
        assert!(features.fetch_ticker);
        assert!(features.create_order);
        assert!(features.ws);
        assert!(features.watch_ticker);
        // Unlisted features should be false
        assert!(!features.swap);
        assert!(!features.future);
        assert!(!features.option);
    }

    #[test]
    fn test_api_urls() {
        let urls: HashMap<String, String> = api_urls! {
            "public" => "https://api.example.com",
            "private" => "https://api.example.com/v1",
        };

        assert_eq!(
            urls.get("public"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(
            urls.get("private"),
            Some(&"https://api.example.com/v1".to_string())
        );
    }

    #[test]
    fn test_exchange_urls() {
        let urls: ExchangeUrls = exchange_urls! {
            logo: "https://example.com/logo.png",
            www: "https://www.example.com",
            api: {
                "public" => "https://api.example.com",
                "private" => "https://api.example.com",
            },
            doc: [
                "https://docs.example.com",
                "https://docs.example.com/v2",
            ],
            fees: "https://example.com/fees",
        };

        assert_eq!(urls.logo, Some("https://example.com/logo.png".to_string()));
        assert_eq!(urls.www, Some("https://www.example.com".to_string()));
        assert_eq!(urls.doc.len(), 2);
        assert_eq!(urls.fees, Some("https://example.com/fees".to_string()));
    }

    #[test]
    fn test_build_query_string() {
        let mut params = std::collections::HashMap::new();
        params.insert("symbol".to_string(), "BTCUSDT".to_string());
        params.insert("limit".to_string(), "100".to_string());

        let query = build_query_string!(params);
        assert!(query.contains("symbol=BTCUSDT"));
        assert!(query.contains("limit=100"));
    }

    #[test]
    fn test_build_query_string_url_encode() {
        let mut params = std::collections::HashMap::new();
        params.insert("symbol".to_string(), "BTC/USDT".to_string());

        let query = build_query_string!(params, url_encode: true);
        assert!(query.contains("symbol=BTC%2FUSDT"));
    }
}
