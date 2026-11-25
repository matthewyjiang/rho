//! A session's pull requests over the socket: clients list them for many
//! sessions at once ([`wire::SESSION_PULL_REQUEST_LIST`]), show one
//! ([`wire::SESSION_PULL_REQUEST_SHOW`]), and link or unlink one
//! ([`wire::SESSION_PULL_REQUEST_LINK`], [`wire::SESSION_PULL_REQUEST_UNLINK`]),
//! through the daemon's one pull request service (see `pull_request_wake`).
//!
//! The session need not be loaded: its workspace is the loaded session's,
//! else the saved one's. A `list` or `show` asks `git` what that workspace
//! has checked out, at most `checkout::RESOLVES_AT_ONCE` at a time, and
//! leases what it names, so the service reads it while clients look. Pull
//! request text is resolved in that workspace by the one resolver the
//! agent's tools use (`resolve_pull_request`). Each handler takes `now`
//! from the daemon host, so tests run them at a fixed time.
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
    reopen::{parse_session, saved_workspace},
    store::store_blocking,
    Sessions,
};
use crate::{
    app::pull_requests::{
        checkout, resolve_pull_request, LinkOrigin, PullRequestError, ShowRead, ShowTarget,
    },
    session::Session as StoredSession,
};

/// The most sessions one `list` names.
//
// The same bound as a `rho/session/list` page (`catalog::MAX_PAGE_SIZE`), so
// a client leases every row of a page in one request. What a full request
// costs, none of it timed at this size: one blocking lookup in the session
// index for the sessions not loaded, which reconciles the index at most once
// when any is missing (its cost grows with the saved store, not with this
// limit); `git` twice for each session found, 8 at a time, at the 3 to 4 ms a
// command measured for `checkout::GIT_DEADLINE`, about 1 s by arithmetic;
// and for the poller, 10 summary queries a host every 5 minutes
// (`gh_query::SUMMARIES_PER_QUERY`), which it logs. A tripwire: more is
// refused with this limit and the asked count. The protocol publishes it as
// the request schema's `maxItems`; this check serves clients that do not
// validate against the schema.
const LISTED_SESSIONS: usize = wire::SessionPullRequestListParams::MAX_SESSIONS;

impl Sessions {
    /// `rho/session/pull_request/list` at `now` (unix milliseconds): each
    /// session asked that is loaded or saved, in the order asked, leased
    /// for the listed cadence. An id that is malformed, or whose lookup
    /// failed, is left out and logged, as one nothing is saved under is.
    pub(in crate::app::daemon_host) async fn pull_request_list(
        &self,
        params: wire::SessionPullRequestListParams,
        now: u64,
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
        let named: Vec<(wire::SessionId, SessionId)> = params
            .sessions
            .into_iter()
            .filter_map(|named| match parse_session(&named) {
                Ok(id) => Some((named, id)),
                Err(_) => {
                    tracing::debug!(session = %named, "left a malformed session id out of a pull request list");
                    None
                }
            })
            .collect();
        let ids: Vec<SessionId> = named.iter().map(|(_, id)| id.clone()).collect();
        let workspaces = self.pull_request_workspaces(&ids).await;
        let (found, workspaces): (Vec<_>, Vec<_>) = named
            .into_iter()
            .zip(workspaces)
            .filter_map(|(named, workspace)| Some((named, workspace?)))
            .unzip();
        let checkouts = checkout::resolve_each(workspaces).await;
        let service = self.pull_requests.service();
        let sessions = found
            .into_iter()
            .zip(checkouts)
            .map(|((named, id), checkout)| {
                service.list(&id, checkout, now);
                wire::ListedSessionPullRequests::new(named, self.tell_pull_requests(&id, now))
            })
            .collect();
        Ok(wire::SessionPullRequestListResponse::new(sessions))
    }

    /// `rho/session/pull_request/show` at `now` (unix milliseconds): the
    /// session's pull requests and the target's detail, leased for the
    /// shown cadence. A linked target's text is resolved as a link's is.
    pub(in crate::app::daemon_host) async fn pull_request_show(
        &self,
        params: wire::SessionPullRequestShowParams,
        now: u64,
    ) -> Result<wire::SessionPullRequestShowResponse, RpcError> {
        let id = parse_session(&params.session)?;
        let read = match params.read {
            wire::PullRequestRead::Cached => ShowRead::Cached,
            wire::PullRequestRead::Fresh => ShowRead::Fresh,
            // Unreachable: the enum has no `Unknown`, so decoding the
            // params refuses a value this daemon does not know.
            _ => return Err(not_known("read")),
        };
        let workspace = self.required_workspace(&id).await?;
        let target = match &params.target {
            wire::PullRequestTarget::Branch => ShowTarget::Branch,
            wire::PullRequestTarget::Linked { pull_request } => ShowTarget::Linked(
                resolve_pull_request(pull_request, &workspace)
                    .await
                    .map_err(refusal)?,
            ),
            // Unreachable, as for `read` above.
            _ => return Err(not_known("target")),
        };
        let checkout = checkout::resolve(&workspace).await;
        let detail = self
            .pull_requests
            .service()
            .show(&id, checkout, target, read, now)
            .await
            .map_err(refusal)?;
        Ok(wire::SessionPullRequestShowResponse::new(
            self.tell_pull_requests(&id, now),
            detail
                .as_ref()
                .map(|detail| Box::new(wire::PullRequestDetail::from(detail))),
        ))
    }

    /// `rho/session/pull_request/link` at `now` (unix milliseconds), as the
    /// person using the client.
    pub(in crate::app::daemon_host) async fn pull_request_link(
        &self,
        params: wire::SessionPullRequestLinkParams,
        now: u64,
    ) -> Result<wire::SessionPullRequestLinkResponse, RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        // Held from the lookup through the save, as a delete holds it until
        // the session's files are gone: a link never saves a record for a
        // session deleted meanwhile.
        let _reopening = self.reopening.lock(&id).await;
        let workspace = self.required_workspace(&id).await?;
        let linked = self
            .pull_requests
            .inbox(&id, &workspace)
            .link(&params.pull_request, LinkOrigin::User, now)
            .await
            .map_err(refusal)?;
        Ok(wire::SessionPullRequestLinkResponse::new(
            wire::PullRequestId::from(&linked.pr),
            self.tell_pull_requests(&id, now),
        ))
    }

    /// `rho/session/pull_request/unlink` at `now` (unix milliseconds),
    /// which ends a watch on it too.
    pub(in crate::app::daemon_host) async fn pull_request_unlink(
        &self,
        params: wire::SessionPullRequestUnlinkParams,
        now: u64,
    ) -> Result<wire::SessionPullRequestUnlinkResponse, RpcError> {
        self.refuse_when_stopping()?;
        let id = parse_session(&params.session)?;
        // As for a link.
        let _reopening = self.reopening.lock(&id).await;
        let workspace = self.required_workspace(&id).await?;
        self.pull_requests
            .inbox(&id, &workspace)
            .unlink(&params.pull_request)
            .await
            .map_err(refusal)?;
        Ok(wire::SessionPullRequestUnlinkResponse::new(
            self.tell_pull_requests(&id, now),
        ))
    }

    /// Tells each loaded session's clients its pull requests whenever they
    /// may have changed, as of `clock` (unix milliseconds), until the
    /// daemon starts stopping. A session loaded since the last change is
    /// told at the next.
    pub(super) async fn report_pull_requests(&self, clock: impl Fn() -> u64) {
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
                    .update_pull_requests(|| service.view(&id, clock()));
            }
            tokio::select! {
                () = self.stopping.cancelled() => return,
                () = changed => {}
            }
        }
    }

    /// Session `id`'s pull requests at `now` (unix milliseconds). A loaded
    /// session's clients are told them first, if they changed.
    fn tell_pull_requests(&self, id: &SessionId, now: u64) -> wire::SessionPullRequests {
        let read = || self.pull_requests.service().view(id, now);
        let loaded = self.loaded().get(id).cloned();
        match loaded {
            Some(session) => session.feed.update_pull_requests(read),
            None => read(),
        }
    }

    /// The workspace of session `id`: the loaded session's, else the saved
    /// one's; `SessionNotFound` when nothing is loaded or saved under it.
    async fn required_workspace(&self, id: &SessionId) -> Result<PathBuf, RpcError> {
        let loaded = self
            .loaded()
            .get(id)
            .map(|session| session.workspace.clone());
        match loaded {
            Some(workspace) => Ok(workspace),
            None => saved_workspace(id).await,
        }
    }

    /// The workspace of each of `ids`, in order: the loaded session's, else
    /// the saved one's, every saved one looked up in one blocking hop that
    /// reconciles the session index at most once. `None` where nothing is
    /// loaded or saved, or where the lookup failed, which is logged.
    async fn pull_request_workspaces(&self, ids: &[SessionId]) -> Vec<Option<PathBuf>> {
        let mut workspaces: Vec<Option<PathBuf>> = {
            let loaded = self.loaded();
            ids.iter()
                .map(|id| loaded.get(id).map(|session| session.workspace.clone()))
                .collect()
        };
        let (slots, unloaded): (Vec<usize>, Vec<String>) = ids
            .iter()
            .enumerate()
            .filter(|(ix, _)| workspaces[*ix].is_none())
            .map(|(ix, id)| (ix, id.as_str().to_owned()))
            .unzip();
        if unloaded.is_empty() {
            return workspaces;
        }
        let rho_home = self.rho_home.clone();
        let saved = store_blocking(
            "look up the sessions a pull request list named",
            move || Ok(StoredSession::workspaces_of(&rho_home, &unloaded)),
        )
        .await;
        let saved = match saved {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(%error, "left the saved sessions out of a pull request list");
                return workspaces;
            }
        };
        for (ix, saved) in slots.into_iter().zip(saved) {
            match saved {
                Ok(workspace) => workspaces[ix] = workspace,
                Err(error) => tracing::warn!(
                    session = %ids[ix],
                    error = format!("{error:#}"),
                    "left a saved session out of a pull request list: its lookup failed"
                ),
            }
        }
        workspaces
    }
}

/// The refusal of a value of `what` this daemon does not know.
fn not_known(what: &str) -> RpcError {
    RpcError::new(
        ErrorCode::InvalidParams,
        format!("this daemon does not know that pull request {what}"),
    )
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
