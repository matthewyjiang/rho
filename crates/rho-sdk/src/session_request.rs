//! Owned request projection for host side requests, shared with orchestration.

use crate::{
    model::{Message, ModelRequest, ToolSpec},
    request_context::ProjectedRequest,
    CancellationToken, Error, ReasoningLevel, Rho, Session,
};

/// An idle session's prepared request inputs, without changing its history.
///
/// This captures advertised tools in registry order, request-only context after
/// preparation, and the session's stored cache key. Append side-request input
/// with [`Self::with_trailing_message`], then borrow it with [`Self::as_request`].
#[derive(Clone, Debug)]
pub struct SessionRequestPrefix {
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    reasoning_level: ReasoningLevel,
    prompt_cache_key: Option<String>,
}

impl SessionRequestPrefix {
    /// Adds input only to this request, never to the session's history.
    pub fn with_trailing_message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Borrows the captured inputs for a provider call.
    pub fn as_request(&self, cancellation: CancellationToken) -> ModelRequest<'_> {
        ModelRequest {
            messages: &self.messages,
            tools: &self.tools,
            cancellation,
            reasoning_level: self.reasoning_level,
            prompt_cache_key: self.prompt_cache_key.as_deref(),
        }
    }
}

impl Session {
    /// Prepares and captures the committed prefix for a host side request.
    ///
    /// Uses the same projection as real turns, including [`crate::RequestContext::prepare`].
    /// Returns [`Error::SessionBusy`] during a run. It does not compact, append
    /// history, execute tools, or send a request. Hosts must keep the session idle
    /// and its provider unchanged until the captured request has been sent.
    pub fn request_prefix(&self) -> Result<SessionRequestPrefix, Error> {
        let _inactive = self.core.lock_inactive()?;
        let runtime = self.core.runtime();
        let history = self.history();
        let tools = runtime.advertised_tool_specs();
        let messages = self.core.project_request(&runtime, &history, &tools);
        Ok(SessionRequestPrefix {
            messages: messages.messages.into_owned(),
            tools,
            reasoning_level: runtime.reasoning_level,
            prompt_cache_key: self.core.prompt_cache_key(),
        })
    }
}

impl super::SessionCore {
    pub(crate) fn project_request<'a>(
        &self,
        runtime: &Rho,
        history: &'a [Message],
        tools: &[ToolSpec],
    ) -> ProjectedRequest<'a> {
        self.prepare_request_context(history, tools);
        runtime.request_messages(self.id(), history)
    }
}

#[cfg(test)]
#[path = "session_request_tests.rs"]
mod tests;
