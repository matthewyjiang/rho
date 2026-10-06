//! Fail-closed selection of one-shot ACP permission options.

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, SelectedPermissionOutcome,
};

/// What an agent policy decided for one `session/request_permission`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermissionDecision {
    /// Select the offered `allow_once` option; never `allow_always` (it persists in the agent's config).
    AllowOnce,
    /// Select `reject_once`, else answer `cancelled`; never `reject_always`.
    Reject,
}

pub(crate) fn choose_option(
    decision: PermissionDecision,
    options: &[PermissionOption],
) -> RequestPermissionOutcome {
    let allowed = match decision {
        PermissionDecision::AllowOnce => options
            .iter()
            .find(|option| option.kind == PermissionOptionKind::AllowOnce),
        PermissionDecision::Reject => None,
    };
    allowed
        .or_else(|| {
            options
                .iter()
                .find(|option| option.kind == PermissionOptionKind::RejectOnce)
        })
        .map_or(RequestPermissionOutcome::Cancelled, |option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        })
}

/// Whether selection actually approved execution, including the missing-allow
/// fallback. Ids themselves carry no policy meaning.
pub(super) fn is_allowed(outcome: &RequestPermissionOutcome, options: &[PermissionOption]) -> bool {
    match outcome {
        RequestPermissionOutcome::Selected(selected) => options.iter().any(|option| {
            option.option_id == selected.option_id && option.kind == PermissionOptionKind::AllowOnce
        }),
        RequestPermissionOutcome::Cancelled => false,
        _ => false,
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
