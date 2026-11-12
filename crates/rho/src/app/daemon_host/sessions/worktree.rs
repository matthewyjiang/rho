//! A new session's git worktree (#1673): the daemon adds it before the
//! session opens (see `crate::worktree`), and, since nothing removes it,
//! names it in the error of a session that then fails to open.

use std::path::Path;

use rho_protocol::{ErrorCode, RpcError, SessionWorktree};

use super::open::Workspace;

/// Adds a worktree under `root` for a session asked to open in
/// `workspace`: the workspace in it the session opens in, and the worktree.
pub(super) async fn add(
    root: &Path,
    workspace: &Workspace,
) -> Result<(Workspace, SessionWorktree), RpcError> {
    let added = crate::worktree::add(root, workspace.path())
        .await
        .map_err(|error| {
            RpcError::new(
                ErrorCode::SessionOpenFailed,
                format!("could not open session in a worktree: {error}"),
            )
        })?;
    tracing::info!(
        branch = %added.worktree.branch,
        path = %added.worktree.path.display(),
        "added a session worktree"
    );
    Ok((Workspace::parse(&added.cwd)?, added.worktree))
}

/// `error`, of a session that failed to open, naming the worktree added for
/// it, which stays.
pub(super) fn kept(mut error: RpcError, worktree: Option<&SessionWorktree>) -> RpcError {
    if let Some(worktree) = worktree {
        error.message = format!(
            "{}; the worktree added for it stays at {} on branch {}",
            error.message,
            worktree.path.display(),
            worktree.branch
        );
    }
    error
}
