use super::{Error, ProviderDiagnostic, ProviderError, ProviderErrorKind, Retryability};

#[test]
fn provider_retryability_propagates_to_top_level_error() {
    let retryable = Error::from(ProviderError::new(
        ProviderErrorKind::RateLimit,
        "rate limited",
        Retryability::Retryable,
    ));
    let permanent = Error::from(ProviderError::new(
        ProviderErrorKind::Authentication,
        "invalid credential",
        Retryability::Permanent,
    ));

    assert!(retryable.is_retryable());
    assert!(!permanent.is_retryable());
}

#[test]
fn provider_diagnostic_is_explicit_and_not_in_display_or_debug() {
    let error = ProviderError::new(
        ProviderErrorKind::InvalidResponse,
        "invalid response",
        Retryability::Permanent,
    )
    .with_diagnostic("secret response body");

    assert_eq!(error.diagnostic(), Some("secret response body"));
    assert!(!error.to_string().contains("secret response body"));
    assert!(!format!("{error:?}").contains("secret response body"));
}

#[test]
fn provider_diagnostics_are_bounded_and_debug_redacted() {
    let diagnostic = ProviderDiagnostic::new("secret".repeat(4_000));

    assert!(diagnostic.as_str().len() <= 16 * 1024);
    assert!(diagnostic.as_str().ends_with("[diagnostic truncated]"));
    assert_eq!(format!("{diagnostic:?}"), "ProviderDiagnostic([redacted])");

    let event = crate::RunEvent::ProviderDiagnostic { detail: diagnostic };
    assert!(!format!("{event:?}").contains("secret"));
}

#[test]
fn format_retry_after_scales_seconds_minutes_and_hours() {
    use std::time::Duration;

    assert_eq!(super::format_retry_after(Duration::from_secs(0)), "now");
    assert_eq!(super::format_retry_after(Duration::from_millis(200)), "1s");
    assert_eq!(super::format_retry_after(Duration::from_secs(12)), "12s");
    assert_eq!(super::format_retry_after(Duration::from_secs(60)), "1m");
    assert_eq!(super::format_retry_after(Duration::from_secs(90)), "1m 30s");
    assert_eq!(super::format_retry_after(Duration::from_secs(3600)), "1h");
    assert_eq!(
        super::format_retry_after(Duration::from_secs(5400)),
        "1h 30m"
    );
}

#[test]
fn provider_error_carries_retry_after() {
    use std::time::Duration;

    let error = ProviderError::new(
        ProviderErrorKind::RateLimit,
        "rate limited",
        Retryability::Retryable,
    )
    .with_retry_after(Duration::from_secs(15));

    assert_eq!(error.retry_after(), Some(Duration::from_secs(15)));
    assert!(format!("{error:?}").contains("15s") || format!("{error:?}").contains("15"));
}
