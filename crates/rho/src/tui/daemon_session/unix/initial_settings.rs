//! The settings a daemon session takes as the TUI opens it: a launch
//! flag's, which must apply, and the ones carried from the session a `/new`
//! replaces, which apply where the daemon takes them.

use rho_protocol::{
    methods::SessionConfigure, ClientError, SessionChange, SessionConfigureParams, SessionId,
    SessionOptions,
};

use crate::app::daemon_client::Link;

/// The changes that move a session from the options `from` to `to`, in
/// the order they apply: a model first, since it decides which
/// reasoning levels and fast mode the others may pick.
pub(super) fn changes_between(from: &SessionOptions, to: &SessionOptions) -> Vec<SessionChange> {
    let mut changes = Vec::new();
    if from.model.current != to.model.current || from.model.auth != to.model.auth {
        changes.push(SessionChange::Model {
            model: to.model.current.clone(),
            auth: Some(to.model.auth.clone()),
        });
    }
    if let Some(reasoning) = to.reasoning.as_ref().filter(|reasoning| {
        from.reasoning.as_ref().map(|from| from.current) != Some(reasoning.current)
    }) {
        changes.push(SessionChange::Reasoning {
            level: reasoning.current,
        });
    }
    let fast =
        |options: &SessionOptions| options.fast_mode.as_ref().is_some_and(|fast| fast.enabled);
    if fast(from) != fast(to) {
        changes.push(SessionChange::FastMode { enabled: fast(to) });
    }
    if from.permission_mode.current != to.permission_mode.current {
        changes.push(SessionChange::PermissionMode {
            mode: to.permission_mode.current,
        });
    }
    changes
}

/// A change the daemon refused, and why.
pub(super) struct Refused {
    pub(super) change: SessionChange,
    pub(super) error: ClientError,
}

/// Applies `changes` to `session` in order, returning its options
/// after the last one that applied, or the first refusal.
pub(super) async fn configure_all(
    link: &Link,
    session: &SessionId,
    changes: Vec<SessionChange>,
) -> Result<Option<SessionOptions>, Box<Refused>> {
    let mut options = None;
    for change in changes {
        let params = SessionConfigureParams {
            session: session.clone(),
            change,
        };
        match link.request::<SessionConfigure>(&params).await {
            Ok(response) => options = Some(response.options),
            Err(error) => {
                return Err(Box::new(Refused {
                    change: params.change,
                    error,
                }))
            }
        }
    }
    Ok(options)
}

/// Carries `changes` to `session`; one the daemon refuses keeps its
/// default. Returns the session's options after the last change that
/// applied, if any did.
pub(super) async fn carry(
    link: &Link,
    session: &SessionId,
    changes: Vec<SessionChange>,
) -> Option<SessionOptions> {
    let mut options = None;
    for change in changes {
        match configure_all(link, session, vec![change]).await {
            Ok(changed) => options = changed.or(options),
            Err(refused) => {
                tracing::warn!(error = %refused.error, "could not carry a setting to the new daemon session");
            }
        }
    }
    options
}
