//! A session's pull requests over the socket: clients list them for many
//! sessions at once ([`wire::SESSION_PULL_REQUEST_LIST`]), show one
//! ([`wire::SESSION_PULL_REQUEST_SHOW`]), and link or unlink one
//! ([`wire::SESSION_PULL_REQUEST_LINK`], [`wire::SESSION_PULL_REQUEST_UNLINK`]),
//! through the daemon's one pull request service (see `pull_request_wake`).
//!
//! The session need not be loaded: its workspace is the loaded session's,
//! else the saved one's. A `list` or `show` asks `git` what that workspace
//! has checked out, at most `checkout::RESOLVES_AT_ONCE` at a time, and
//! leases what it names, so the service reads it while clients look.
//!
//! A loaded session's clients follow its pull requests as
//! [`wire::SessionEvent::PullRequests`]: one reporter tells every loaded
//! session's feed whenever the service says they may have changed, and
//! each request here tells the session's feed before it answers, so what a
//! client just read is what the feed last sent. The feed sends only a set
//! that differs from the last.

use std::{path::PathBuf, sync::Arc};

use rho_protocol::{self as wire, ErrorCode, RpcError};
use rho_sdk::SessionId;

use super::{
    reopen::{find_saved_workspace, not_found, parse_session},
    Sessions,
};
use crate::app::pull_requests::{
    checkout, unix_ms, LinkOrigin, PullRequestError, ShowRead, ShowTarget,
};

/// The most sessions one `list` names.
//
// The same bound as a `rho/session/list` page (`catalog::MAX_PAGE_SIZE`), so
// a client leases every row of a page in one request. At this size the
// request asks `git` 2,000 times, 3 to 4 ms each and 8 at a time, about 1 s,
// and the poller needs 10 summary queries a host every 5 minutes
// (`gh_query::SUMMARIES_PER_QUERY`), which it logs. A tripwire: more is
// refused with this limit and the asked count.
const LISTED_SESSIONS: usize = 1_000;

impl Sessions {
    /// `rho/session/pull_request/list`: each session asked that is loaded
    /// or saved, in the order asked, leased for the listed cadence.
    pub(in crate::app::daemon_host) async fn pull_request_list(
        &self,
        params: wire::SessionPullRequestListParams,
    ) -> Result<wire::SessionPullRequestListResponse, RpcError> {
        let asked = params.sessions.len();
        if asked > LISTED_SESSIONS {
            return Err(RpcError::new(
                ErrorCode::InvalidParams,
                format!(
                    "{asked} sessions is over the pull request list's limit (LISTED_SESSIONS = {LISTED_SESSIONS})"
                ),
            ));
        }
        // One at a time: a miss rebuilds the session index.
        let mut found = Vec::with_capacity(asked);
        for named in params.sessions {
            let id = parse_session(&named)?;
            if let Some(workspace) = self.pull_request_workspace(&id).await? {
                found.push((named, id, workspace));
            }
        }
        let workspaces = found
            .iter()
            .map(|(_, _, workspace)| Some(workspace.clone()))
            .collect();
        let checkouts = checkout::resolve_each(workspaces).await;
        let service = self.pull_requests.service();
        let now = unix_ms();
        let sessions = found
            .into_iter()
            .zip(checkouts)
            .map(|((named, id, _), checkout)| {
                service.list(&id, checkout, now);
                wire::ListedSessionPullRequests::new(named, self.tell_pull_requests(&id))
            })
            .collect();
        Ok(wire::SessionPullRequestListResponse::new(sessions))
    }

    /// `rho/session/pull_request/show`: the session's pull requests and the
    /// target's detail, leased for the shown cadence.
    pub(in crate::app::daemon_host) async fn pull_request_show(
        &self,
        params: wire::SessionPullRequestShowParams,
    ) -> Result<wire::SessionPullRequestShowResponse, RpcError> {
        let id = parse_session(&params.session)?;
        let target = match &params.target {
            wire::PullRequestTarget::Branch => ShowTarget::Branch,
            wire::PullRequestTarget::Linked { pull_request } => ShowTarget::Linked(pull_request),
            _ => {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    "this daemon does not know that kind of pull request target",
                ))
            }
        };
        let read = match params.read {
            wire::PullRequestRead::Cached => ShowRead::Cached,
            wire::PullRequestRead::Fresh => ShowRead::Fresh,
            // A read only a newer client names answers from the last read.
            _ => ShowRead::Cached,
        };
        let workspace = self
            .pull_request_workspace(&id)
            .await?
            .ok_or_else(|| not_found(&id))?;
        let checkout = checkout::resolve(&workspace).await;
        let detail = self
            .pull_requests
            .service()
            .show(&id, Some(checkout), target, read, unix_ms())
            .await
            .map_err(refusal)?;
        Ok(wire::SessionPullRequestShowResponse::new(
            self.tell_pull_requests(&id),
            detail
                .as_ref()
                .map(|detail| Box::new(wire::PullRequestDetail::from(detail))),
        ))
    }

    /// `rho/session/pull_request/link`, as the person using the client.
    pub(in crate::app::daemon_host) async fn pull_request_link(
        &self,
        params: wire::SessionPullRequestLinkParams,
    ) -> Result<wire::SessionPullRequestLinkResponse, RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        let workspace = self
            .pull_request_workspace(&id)
            .await?
            .ok_or_else(|| not_found(&id))?;
        let linked = self
            .pull_requests
            .inbox(&id, &workspace)
            .link(&params.pull_request, LinkOrigin::User, unix_ms())
            .await
            .map_err(refusal)?;
        Ok(wire::SessionPullRequestLinkResponse::new(
            wire::PullRequestId::from(&linked.pr),
            self.tell_pull_requests(&id),
        ))
    }

    /// `rho/session/pull_request/unlink`, which ends a watch on it too.
    pub(in crate::app::daemon_host) async fn pull_request_unlink(
        &self,
        params: wire::SessionPullRequestUnlinkParams,
    ) -> Result<wire::SessionPullRequestUnlinkResponse, RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        let workspace = self
            .pull_request_workspace(&id)
            .await?
            .ok_or_else(|| not_found(&id))?;
        self.pull_requests
            .inbox(&id, &workspace)
            .unlink(&params.pull_request)
            .await
            .map_err(refusal)?;
        Ok(wire::SessionPullRequestUnlinkResponse::new(
            self.tell_pull_requests(&id),
        ))
    }

    /// Tells each loaded session's clients its pull requests whenever they
    /// may have changed, until the daemon starts stopping. A session loaded
    /// since the last change is told at the next.
    pub(super) async fn report_pull_requests(&self) {
        let service = self.pull_requests.service();
        loop {
            // Before the reads, so a change during them wakes the next pass.
            let changed = service.changes();
            // Copied out, so the daemon-wide map is not held under a feed.
            let loaded: Vec<_> = self
                .loaded()
                .iter()
                .map(|(id, session)| (id.clone(), Arc::clone(session)))
                .collect();
            for (id, session) in loaded {
                session
                    .feed
                    .update_pull_requests(|| service.view(&id, unix_ms()));
            }
            tokio::select! {
                () = self.stopping.cancelled() => return,
                () = changed => {}
            }
        }
    }

    /// Session `id`'s pull requests now. A loaded session's clients are
    /// told them first, if they changed.
    fn tell_pull_requests(&self, id: &SessionId) -> wire::SessionPullRequests {
        let read = || self.pull_requests.service().view(id, unix_ms());
        let loaded = self.loaded().get(id).cloned();
        match loaded {
            Some(session) => session.feed.update_pull_requests(read),
            None => read(),
        }
    }

    /// The workspace of session `id`: the loaded session's, else the saved
    /// one's; `None` when nothing is loaded or saved under it.
    async fn pull_request_workspace(&self, id: &SessionId) -> Result<Option<PathBuf>, RpcError> {
        let loaded = self
            .loaded()
            .get(id)
            .map(|session| session.workspace.clone());
        match loaded {
            Some(workspace) => Ok(Some(workspace)),
            None => find_saved_workspace(id).await,
        }
    }
}

/// The error a request answers when the service refused it.
fn refusal(error: PullRequestError) -> RpcError {
    let code = match &error {
        PullRequestError::InvalidTarget(_) => ErrorCode::InvalidParams,
        PullRequestError::Store(_) => ErrorCode::InternalError,
        // Only a watch reads the forge before it answers, or counts
        // watches; a show keeps what it could not read as trouble.
        PullRequestError::AlreadyEnded { .. }
        | PullRequestError::NotInstalled
        | PullRequestError::SignedOut { .. }
        | PullRequestError::Budget { .. }
        | PullRequestError::TooManyWatches { .. }
        | PullRequestError::NotFound(_)
        | PullRequestError::Read(_) => ErrorCode::InternalError,
    };
    RpcError::new(code, error.to_string())
}

#[cfg(test)]
#[path = "pull_requests_tests.rs"]
mod tests;
