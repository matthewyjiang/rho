//! Session marks over the socket (`rho/session/mark`), and the one the
//! daemon sets itself: a session is marked done when a pull request its
//! agent watched merges and it watches no other.
//!
//! A mark is saved with the session, beside its title (see
//! `crate::session`'s `mark`), so it needs no loaded session, and
//! `rho/session/list_detailed` reports it. A change tells a loaded session's
//! attached clients, as a rename does.
//!
//! Only the daemon sees a merge: the watch ends with it, and clients then
//! see a linked pull request nobody watches. The session is marked done
//! twice. Once at once, as the poll that read the merge queues its news
//! ([`Sessions::mark_merged_done`]), which stands while no turn tells the
//! agent; a daemon that stops between the two never makes this mark. And
//! again as the turn that told the agent ends ([`turn_ended`]): that
//! turn's saves moved the session's `updated_at` past the first mark, and
//! clients read a session as done only while `updated_at` is not after
//! it. A prompt the person sends later moves `updated_at` past the mark
//! again, which clients read as active; the mark stays, and the turn of
//! such a prompt marks nothing, also when it is the one to tell the agent
//! news that waited.
//!
//! A merge that leaves the session watching another pull request marks
//! nothing either time: its work is not over, and a session marked done
//! by mistake is hidden, which is worse than one left listed. Neither
//! does a merge beside a watch that ended some other way, in the same
//! read or since. Past that,
//! [`after_merge`] is the whole rule; a watch that ends any other way
//! marks nothing.

use rho_protocol::{self as wire, ErrorCode, RpcError};
use rho_sdk::SessionId;

use super::{
    reopen::{not_found, parse_session},
    store::store_blocking,
    LoadedSession, Sessions,
};
use crate::{
    app::{
        pull_requests::{MergedWatch, PullRequestInbox},
        session_runtime::SessionRuntime,
    },
    session::{MarkChange, NewMark, Session as StoredSession, SessionMark},
};

impl Sessions {
    /// Saves or clears a session's mark, then tells the clients attached
    /// to it, if the daemon has it loaded.
    pub(in crate::app::daemon_host) async fn mark(
        &self,
        params: wire::SessionMarkParams,
    ) -> Result<wire::SessionMarkResponse, RpcError> {
        let id = parse_session(&params.session)?;
        let asked = params.mark.map(new_mark).transpose()?;
        let change = self
            .change_saved_mark(&id, move |_, now| asked.map(|mark| mark.at(now)))
            .await?
            .ok_or_else(|| not_found(&id))?;
        self.tell_mark(&id, change);
        Ok(wire::SessionMarkResponse::new(
            change.after.map(mark_to_wire),
        ))
    }

    /// Marks done each session whose last watch ended because its pull
    /// request merged, as [`after_merge`] says, until the daemon starts
    /// stopping: a merge it read before that is marked first.
    pub(super) async fn mark_merged_done(&self) {
        let Some(mut merged) = self.pull_requests.service().take_merged_watches() else {
            return;
        };
        loop {
            let MergedWatch { session, at } = tokio::select! {
                biased;
                Some(merged) = merged.recv() => merged,
                () = self.stopping.cancelled() => return,
                else => return,
            };
            let merged_at = at / 1000;
            let change = self
                .change_saved_mark(&session, move |mark, now| after_merge(mark, merged_at, now));
            match change.await {
                Ok(Some(change)) => self.tell_mark(&session, change),
                // Deleted since, or it never saved a turn.
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    %session,
                    %error,
                    "could not mark a session done for its merged pull request"
                ),
            }
        }
    }

    /// Replaces the mark of the session saved under `id`, loaded or not,
    /// with what `change` makes of the one it has and of the time now on
    /// the session store's clock; `None` when nothing is saved under `id`.
    async fn change_saved_mark(
        &self,
        id: &SessionId,
        change: impl FnOnce(Option<SessionMark>, u64) -> Option<SessionMark> + Send + 'static,
    ) -> Result<Option<MarkChange>, RpcError> {
        let (rho_home, id) = (self.rho_home.clone(), id.clone());
        store_blocking("save the session's mark", move || {
            StoredSession::change_mark_of(&rho_home, id.as_str(), change)
        })
        .await
    }

    /// Tells the clients attached to session `id`, if it is loaded, the
    /// mark `change` left, if it changed it.
    fn tell_mark(&self, id: &SessionId, change: MarkChange) {
        let loaded = self.loaded().get(id).cloned();
        if let Some(session) = loaded {
            tell_mark(&session, change);
        }
    }
}

/// What started a turn, which decides whether its end marks the session
/// (see [`turn_ended`]).
#[derive(Clone, Copy)]
pub(super) enum TurnOrigin {
    /// The daemon, to tell the agent what waited for it.
    Daemon,
    /// A prompt sent at `at`, in unix milliseconds.
    Prompt { at: u64 },
}

/// Call as `session`'s turn on `runtime`, which `origin` started, ended,
/// after its last save and before its end is told. A turn that told the
/// agent a watched pull request merged, and leaves the session watching
/// none, marks the session done again, as [`after_merge`] says: at or
/// after the `updated_at` the turn leaves, so clients read the session as
/// done once the turn is over. Not the turn of
/// a prompt sent since rho found the merge, as when the news waited for
/// one: the person turned to the session again, so it stays active.
pub(super) fn turn_ended(session: &LoadedSession, runtime: &SessionRuntime, origin: TurnOrigin) {
    let told = runtime
        .tools()
        .pull_requests()
        .and_then(PullRequestInbox::take_merge_told);
    let Some(merged_at) = told else {
        return;
    };
    match origin {
        TurnOrigin::Daemon => {}
        // The merge came while the prompt's turn ran.
        TurnOrigin::Prompt { at } if at <= merged_at => {}
        TurnOrigin::Prompt { .. } => return,
    }
    let merged_at = merged_at / 1000;
    match runtime.change_mark(|mark, now| after_merge(mark, merged_at, now)) {
        Ok(Some(change)) => tell_mark(session, change),
        // It keeps no file.
        Ok(None) => {}
        Err(error) => tracing::warn!(
            session = %session.id,
            error = format!("{error:#}"),
            "could not mark a session done after the turn that told of its merged pull request"
        ),
    }
}

/// The mark of a session whose watched pull request rho found merged at
/// `merged_at`, as of `now`, both unix seconds, given the `mark` it has:
/// done as of `now`, unless the person set it apart. A pinned session
/// stays pinned, and a snoozed one snoozed until its snooze ends. One the
/// person put back on the active list since the merge stays there, so the
/// second marking, at the end of the turn that tells the agent, does not
/// undo a restore made while that turn ran.
fn after_merge(mark: Option<SessionMark>, merged_at: u64, now: u64) -> Option<SessionMark> {
    match mark {
        Some(SessionMark::Pinned { .. }) => mark,
        Some(SessionMark::Snoozed { until }) if until > now => mark,
        Some(SessionMark::Kept { at }) if at >= merged_at => mark,
        None
        | Some(SessionMark::Done { .. } | SessionMark::Snoozed { .. } | SessionMark::Kept { .. }) => {
            Some(SessionMark::Done { at: now })
        }
    }
}

/// Tells `session`'s attached clients the mark `change` left, if it
/// changed it.
fn tell_mark(session: &LoadedSession, change: MarkChange) {
    if change.before != change.after {
        session.feed.publish([wire::SessionEvent::MarkChanged {
            mark: change.after.map(mark_to_wire),
        }]);
    }
}

/// The mark `rho/session/mark` asks for, which the store stamps.
fn new_mark(mark: wire::NewSessionMark) -> Result<NewMark, RpcError> {
    match mark {
        wire::NewSessionMark::Done => Ok(NewMark::Done),
        wire::NewSessionMark::Snoozed { until } => Ok(NewMark::Snoozed { until }),
        wire::NewSessionMark::Pinned => Ok(NewMark::Pinned),
        wire::NewSessionMark::Kept => Ok(NewMark::Kept),
        // Unreachable: this build's protocol crate decoded it, so it knows
        // every kind; the enum is non-exhaustive for clients.
        _ => Err(RpcError::new(
            ErrorCode::InvalidParams,
            "this daemon does not know that kind of mark",
        )),
    }
}

/// `mark` as `rho/session/list_detailed` and `mark_changed` send it.
pub(super) fn mark_to_wire(mark: SessionMark) -> wire::SessionMark {
    match mark {
        SessionMark::Done { at } => wire::SessionMark::Done { at },
        SessionMark::Snoozed { until } => wire::SessionMark::Snoozed { until },
        SessionMark::Pinned { at } => wire::SessionMark::Pinned { at },
        SessionMark::Kept { at } => wire::SessionMark::Kept { at },
    }
}

#[cfg(test)]
#[path = "marks_tests.rs"]
mod tests;
