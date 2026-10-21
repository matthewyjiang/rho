//! Provider-reported reasons a model turn ended early.

use serde_json::Value;

use super::ModelEvent;

// NEXT_MAJOR(rho-sdk): replace the stop-reason ProviderContext marker with a
// typed `ModelEvent::Stopped(ModelStopReason)` variant.
/// Reserved [`ModelEvent::ProviderContext`] kind that carries a [`ModelStopReason`].
pub(crate) const STOP_REASON_CONTEXT_KIND: &str = "rho.sdk.stop_reason.v1";

/// Why a provider ended one model turn other than by the model finishing.
///
/// Providers report it with [`ModelEvent::stop_reason`]. A turn without a
/// report finished normally. The runtime surfaces the reason of the last model
/// turn in a run as [`crate::StopReason`].
///
/// # Next major
///
/// NEXT_MAJOR(rho-sdk): replace the stop-reason ProviderContext marker with a
/// typed `ModelEvent::Stopped(ModelStopReason)` variant.
///
/// `ModelEvent` and `ModelResponse` cannot grow in a minor, so the reason
/// travels as SDK metadata on a reserved provider-context kind. Build it with
/// [`ModelEvent::stop_reason`] and read it with [`ModelEvent::as_stop_reason`];
/// do not match the encoding directly.
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

impl ModelStopReason {
    fn wire_name(self) -> &'static str {
        match self {
            Self::MaxTokens => "max_tokens",
            Self::Refusal => "refusal",
        }
    }
}

impl ModelEvent {
    /// Reports why the provider ended the current model turn.
    ///
    /// Send it once the reason is known; if a turn reports more than once, the
    /// last report wins. The runtime consumes the event; it never reaches
    /// session history or [`crate::RunEvent::ProviderContextUpdated`].
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-sdk): replace the stop-reason ProviderContext marker with a
    /// typed `ModelEvent::Stopped(ModelStopReason)` variant.
    pub fn stop_reason(reason: ModelStopReason) -> Self {
        Self::ProviderContext {
            kind: STOP_REASON_CONTEXT_KIND.into(),
            position: None,
            data: Value::String(reason.wire_name().into()),
        }
    }

    /// Returns the reason when this event was built by [`Self::stop_reason`].
    ///
    /// A hand-built event with the reserved kind but other data returns `None`.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-sdk): replace the stop-reason ProviderContext marker with a
    /// typed `ModelEvent::Stopped(ModelStopReason)` variant.
    pub fn as_stop_reason(&self) -> Option<ModelStopReason> {
        let Self::ProviderContext {
            kind,
            position: None,
            data,
        } = self
        else {
            return None;
        };
        if kind != STOP_REASON_CONTEXT_KIND {
            return None;
        }
        match data.as_str()? {
            "max_tokens" => Some(ModelStopReason::MaxTokens),
            "refusal" => Some(ModelStopReason::Refusal),
            _ => None,
        }
    }
}
