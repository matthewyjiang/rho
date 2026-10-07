use std::borrow::Cow;

use crate::{
    model::{Message, ToolSpec},
    ContextEstimate, Rho, SessionId,
};

/// Host-owned context projected into model requests, not conversation history.
///
/// Implementors return current provider-neutral context for the requested session.
/// The SDK appends it to provider requests and includes it in context accounting;
/// it is never summarized, persisted as a conversation entry, or treated as human
/// input. Hosts own persistence and must restore their source before resuming.
/// Sources must be synchronous, cheap, and must not read or mutate SDK sessions.
/// Prefer [`Message::model_context`] for data messages so provider-side prompt
/// readers can distinguish their attribution from a new human request.
/// Compactors receive raw conversation history; hosts must reserve the context
/// footprint when sizing replacements, since this mandatory data is not summarized.
pub trait RequestContext: Send + Sync {
    /// Refreshes host-owned request context for an upcoming history boundary.
    ///
    /// The SDK supplies the actual conversation history (including pending user
    /// input), advertised tool schemas, and its current estimate, including any
    /// applicable provider calibration and the currently sampled request context.
    /// Hosts may use this data to size their next [`Self::messages`] projection;
    /// the SDK does not choose host context policy.
    ///
    /// Called before automatic compaction evaluation and before provider request
    /// projection, including overflow recovery retries and history updated by
    /// late boundary input or staged steering. It may run more than once for the
    /// same boundary, so implementations should be idempotent. Read-only idle
    /// accounting APIs do not invoke it.
    ///
    /// This callback must be synchronous and cheap, and must not call back into
    /// SDK sessions or acquire their locks. The SDK releases its session locks
    /// before invoking it, then samples [`Self::messages`] again for projection.
    #[allow(unused_variables)]
    fn prepare(
        &self,
        session_id: &SessionId,
        history: &[Message],
        tools: &[ToolSpec],
        estimate: ContextEstimate,
    ) {
    }

    fn messages(&self, session_id: &SessionId) -> Vec<Message>;
}

#[cfg(test)]
#[path = "request_context_tests.rs"]
mod tests;

pub(crate) struct ProjectedRequest<'a> {
    pub messages: Cow<'a, [Message]>,
    /// Keep the exact source sampled for this request, even if the live source
    /// changes while the provider is running.
    pub context: Vec<Message>,
}

impl Rho {
    pub(crate) fn context_messages(&self, session_id: &SessionId) -> Vec<Message> {
        self.request_context
            .as_ref()
            .map(|source| source.messages(session_id))
            .unwrap_or_default()
    }

    pub(crate) fn request_messages<'a>(
        &self,
        session_id: &SessionId,
        history: &'a [Message],
    ) -> ProjectedRequest<'a> {
        let context = self.context_messages(session_id);
        if context.is_empty() {
            return ProjectedRequest {
                messages: Cow::Borrowed(history),
                context,
            };
        }
        let mut messages = history.to_vec();
        messages.extend_from_slice(&context);
        ProjectedRequest {
            messages: Cow::Owned(messages),
            context,
        }
    }
}
