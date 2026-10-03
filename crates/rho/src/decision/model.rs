//! The model a feature asks its decision requests of.

use futures_util::future::BoxFuture;
use rho_sdk::CancellationToken;

use super::{Answers, DecisionRequest};

/// Answers [`DecisionRequest`]s: a decision model over the System One API,
/// or a text model through [`super::llm::TextModel`]. A feature written
/// against this trait runs on either.
///
/// Implementors answer every question with one of its options or return an
/// error, never a default answer, so a caller can fail closed. A model that
/// reports probabilities returns them for the options it scored, each within
/// 0 to 1; one that does not returns `None` rather than inventing them.
pub(crate) trait DecisionModel: Send + Sync {
    /// Answers `request`, or errors when the model cannot or when
    /// `cancellation` fires first.
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        cancellation: &'a CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<Answers>>;

    /// Largest state the model accepts, in Rho's chars/4 token estimate, or
    /// `None` when the caller sizes the state for the model itself. A caller
    /// with a longer state shortens it first.
    fn state_budget(&self) -> Option<u64>;
}
