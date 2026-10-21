//! Provider-reported reasons a model turn ended early.

use serde_json::Value;

use super::ModelEvent;

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

    /// Parses marker data. Unknown names come from a newer producer and are ignored.
    pub(crate) fn from_marker_data(data: &Value) -> Option<Self> {
        match data.as_str()? {
            "max_tokens" => Some(Self::MaxTokens),
            "refusal" => Some(Self::Refusal),
            _ => None,
        }
    }
}

impl ModelEvent {
    /// Reports why the provider ended the current model turn.
    ///
    /// Send it at most once per turn, before returning the response. The
    /// runtime consumes the event; it never reaches session history or
    /// [`crate::RunEvent::ProviderContextUpdated`].
    pub fn stop_reason(reason: ModelStopReason) -> Self {
        Self::ProviderContext {
            kind: STOP_REASON_CONTEXT_KIND.into(),
            position: None,
            data: Value::String(reason.wire_name().into()),
        }
    }

    /// Returns the reason when this event was built by [`Self::stop_reason`].
    pub fn as_stop_reason(&self) -> Option<ModelStopReason> {
        match self {
            Self::ProviderContext {
                kind,
                position: None,
                data,
            } if kind == STOP_REASON_CONTEXT_KIND => ModelStopReason::from_marker_data(data),
            _ => None,
        }
    }
}
