//! The session store, from a request: its blocking work run off the async
//! runtime, and its failures as the errors a client acts on.

use rho_protocol::{ErrorCode, RpcError};

use crate::session::{RunNotFinished, SavedInManyWorkspaces, SessionInUse, TitleRejected};

/// Runs session-store work `work`, which reads SQLite and walks session
/// directories, off the async runtime; its failure as [`store_failed`]
/// says, naming `what` it could not do.
pub(super) async fn store_blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, RpcError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(store_failed(what, &error)),
        Err(error) => Err(store_failed(what, &error.into())),
    }
}

/// The error a request answers when the session store failed it with
/// `error`, wherever in its chain the store's refusal is, as a child
/// session's is under its parent's: a session something else holds is
/// `SessionInUse`, a run a delete must wait for `SessionBusy`, a refused
/// title or an id saved in more than one workspace `InvalidParams` (the
/// client chose them), and anything else `InternalError` saying the daemon
/// could not do `what`.
pub(super) fn store_failed(what: &str, error: &anyhow::Error) -> RpcError {
    let refused = |cause: &&(dyn std::error::Error + 'static)| {
        cause.is::<TitleRejected>() || cause.is::<SavedInManyWorkspaces>()
    };
    if error.chain().any(|cause| cause.is::<SessionInUse>()) {
        return RpcError::new(ErrorCode::SessionInUse, format!("{error:#}"));
    }
    if let Some(run) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<RunNotFinished>())
    {
        // The store's own wording offers `rho sessions rm --force`, which
        // the socket has no counterpart of.
        return RpcError::new(
            ErrorCode::SessionBusy,
            format!(
                "run {} of session {} is still {}; delete the session once the run ends",
                run.run(),
                run.session(),
                run.state()
            ),
        );
    }
    match error.chain().find(refused) {
        Some(cause) => RpcError::new(ErrorCode::InvalidParams, cause.to_string()),
        None => RpcError::new(
            ErrorCode::InternalError,
            format!("could not {what}: {error:#}"),
        ),
    }
}
