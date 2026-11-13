//! Provider-reported reasons a model turn ended early.

/// Why a provider ended one model turn other than by the model finishing.
///
/// Providers report it with [`super::ModelEvent::Stopped`]. A turn without a
/// report finished normally. The runtime surfaces the reason of the last model
/// turn in a run as [`crate::StopReason`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelStopReason {
    /// Output reached the provider's output-token or context-window limit, so
    /// the turn may be truncated.
    MaxTokens,
    /// The provider refused the request or filtered the output for policy or
    /// safety reasons.
    Refusal,
}
