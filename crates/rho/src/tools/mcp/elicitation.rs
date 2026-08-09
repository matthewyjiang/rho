//! Answering `elicitation/create`: a server asking the user a question.
//!
//! Rho shows the request as an ordinary questionnaire on the tool card that
//! provoked it, which is why the answer is routed through the in-flight
//! `tools/call` (see [`super::inflight`]) rather than through any session-wide
//! channel: a questionnaire only reaches a person while a turn is running.
//!
//! Every path that cannot put the question in front of a person **declines**.
//! Declining is a first-class MCP answer and lets the server carry on without
//! the information; inventing an answer would not.

use rho_sdk::HostInputRequest;
use rmcp::{
    model::{ElicitRequestParams, ElicitResult, ElicitationAction},
    ErrorData as McpError,
};

use super::{elicitation_form::ElicitationForm, inflight::McpInFlightCalls};

/// Whether this run can put a question in front of a person.
///
/// Rho declares the `elicitation` capability only when this is
/// [`Self::Available`], because a run with no questionnaire loop (`rho mcp`
/// inventory, an automation run started without a host input responder) would
/// have to decline every request it invited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpElicitationSupport {
    Available,
    Unavailable,
}

impl McpElicitationSupport {
    pub(crate) fn is_available(self) -> bool {
        match self {
            Self::Available => true,
            Self::Unavailable => false,
        }
    }
}

/// Serves one session's elicitation requests.
#[derive(Clone, Debug)]
pub(crate) struct McpElicitationService {
    identity: String,
    calls: McpInFlightCalls,
    support: McpElicitationSupport,
}

impl McpElicitationService {
    pub(crate) fn new(
        identity: impl Into<String>,
        calls: McpInFlightCalls,
        support: McpElicitationSupport,
    ) -> Self {
        Self {
            identity: identity.into(),
            calls,
            support,
        }
    }

    /// Whether the `elicitation` capability may be declared for this session.
    pub(crate) fn is_available(&self) -> bool {
        self.support.is_available()
    }

    /// Answer one `elicitation/create`.
    ///
    /// Returns `Ok` for every outcome the protocol has an action for, and only
    /// fails the request when the server asked for something Rho does not model.
    pub(crate) async fn elicit(
        &self,
        request: ElicitRequestParams,
    ) -> Result<ElicitResult, McpError> {
        // A server that asks without seeing the capability declared is answered
        // rather than obeyed. Sending the question on would reach a run with no
        // questionnaire loop, where an unanswerable request fails the whole run.
        if !self.support.is_available() {
            return Ok(self.decline("this Rho run cannot show a server's question to anyone"));
        }
        let (message, schema) = match request {
            ElicitRequestParams::FormElicitationParams {
                message,
                requested_schema,
                ..
            } => (message, requested_schema),
            // URL mode asks Rho to send the user to a web page. Rho has no
            // browser affordance it can drive safely from a background session,
            // and accepting without opening anything would tell the server the
            // user had answered.
            ElicitRequestParams::UrlElicitationParams { .. } => {
                return Ok(self.decline("Rho does not support URL elicitation"))
            }
            // Non-exhaustive upstream: an unknown mode is not something Rho can
            // show, so it declines instead of guessing.
            _ => return Ok(self.decline("Rho does not support this elicitation mode")),
        };

        let caller = match self.calls.sole_caller() {
            Ok(caller) => caller,
            Err(error) => return Ok(self.decline(error.reason())),
        };
        let form = match ElicitationForm::from_schema(&schema) {
            Ok(form) => form,
            Err(error) => return Ok(self.decline(error.reason())),
        };
        let title = elicitation_title(&self.identity, &message);
        let host_request = match HostInputRequest::questionnaire(title, form.questions().to_vec()) {
            Ok(request) => request,
            Err(error) => return Ok(self.decline(error.to_string())),
        };
        match caller.ask(host_request).await {
            Ok(response) => match form.content(&response) {
                Ok(content) => {
                    Ok(ElicitResult::new(ElicitationAction::Accept).with_content(content))
                }
                // The user answered, but the answer cannot be typed the way the
                // schema declares. Sending it anyway would break the server's
                // contract, so the request is declined instead.
                Err(error) => Ok(self.decline(error.reason())),
            },
            // Dismissing the form cancels the turn, so cancelling the whole
            // operation is exactly what happened.
            Err(rho_sdk::Error::Cancelled) => Ok(ElicitResult::new(ElicitationAction::Cancel)),
            Err(error) => Ok(self.decline(error.to_string())),
        }
    }

    /// A decline plus the reason, at debug level only: elicitation messages are
    /// server-authored prose that can carry whatever the server put in them.
    fn decline(&self, reason: impl AsRef<str>) -> ElicitResult {
        tracing::debug!(
            server = %self.identity,
            reason = reason.as_ref(),
            "declined an MCP elicitation request"
        );
        ElicitResult::new(ElicitationAction::Decline)
    }
}

/// Name the asking server in the form title, so a question that interrupts a
/// turn is never mistaken for one of Rho's own.
fn elicitation_title(identity: &str, message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        return format!("MCP server `{identity}` needs input");
    }
    format!("MCP server `{identity}`: {message}")
}

#[cfg(test)]
#[path = "elicitation_tests.rs"]
mod tests;
