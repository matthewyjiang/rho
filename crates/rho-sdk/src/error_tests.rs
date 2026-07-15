use super::{Error, ProviderError, ProviderErrorKind, Retryability};

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
fn provider_debug_and_display_contain_only_sanitized_fields() {
    let error = ProviderError::new(
        ProviderErrorKind::Unavailable,
        "service unavailable",
        Retryability::Retryable,
    );

    assert_eq!(error.to_string(), "provider failed: service unavailable");
    assert_eq!(
        format!("{error:?}"),
        "ProviderError { kind: Unavailable, message: \"service unavailable\", retryability: Retryable }"
    );
}
