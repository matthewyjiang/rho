//! Cancellation for individual operations inside a provider turn.
//!
//! Turn-level cancellation belongs to the SDK adapter (see
//! `crate::impl_sdk_model_provider!`). Transports use this helper only for work
//! that should abort on its own, such as a single in-flight HTTP request or a
//! credential refresh, and only where the token is optional at the call site.

use rho_sdk::CancellationToken;

use crate::model::ModelError;

/// Awaits `future`, resolving to [`ModelError::Interrupted`] if `cancellation`
/// fires first. A `None` token awaits the future unchanged.
pub(crate) async fn cancel_aware<T>(
    cancellation: Option<&CancellationToken>,
    future: impl std::future::Future<Output = Result<T, ModelError>>,
) -> Result<T, ModelError> {
    match cancellation {
        Some(cancellation) => tokio::select! {
            result = future => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        },
        None => future.await,
    }
}
