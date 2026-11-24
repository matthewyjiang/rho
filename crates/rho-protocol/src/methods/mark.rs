//! Session marks: how a person has organised their saved sessions, kept
//! with the session by the daemon, so every frontend of one daemon shows
//! the same list. A session has at most one mark: done, snoozed, pinned,
//! or kept. [`super::SESSION_LIST_DETAILED`] reports it and
//! [`SESSION_MARK`] sets or clears it.
//!
//! Every time here is unix seconds on the daemon's clock, the clock of
//! [`super::SessionSummary::updated_at`], and the daemon stamps each `at`
//! itself, so a frontend compares the two whatever its own clock says: a
//! session marked done is finished while `updated_at <= at`, and active
//! again once a later save moves `updated_at` past it. A mark never moves
//! `updated_at`, and no save clears a mark.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// Set or clear a saved session's mark. A daemon from before this method
/// answers [`crate::ErrorCode::MethodNotFound`], and one that has
/// [`super::SESSION_LIST_DETAILED`] lists no marks with it: only this
/// answer tells such a daemon from one whose sessions have none.
pub const SESSION_MARK: &str = "rho/session/mark";

/// A session's mark, as the daemon saved it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionMark {
    /// The person is finished with the session, as of `at`. The daemon
    /// marks a session done itself when a pull request its agent watched
    /// merges and it watches no other, at or after the last save of the
    /// turn that told the agent.
    Done { at: u64 },
    /// Hide the session until `until`, which the client chose.
    Snoozed { until: u64 },
    /// Keep the session on top; pinned at `at`.
    Pinned { at: u64 },
    /// The person put the session back on the active list at `at`, as by
    /// restoring a done one or unpinning a pinned one.
    Kept { at: u64 },
    /// A mark a newer daemon saved that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The mark [`SESSION_MARK`] sets, each kind as [`SessionMark`]'s of the
/// same name. The daemon stamps the time a mark is `at`; only a snooze's
/// end is the client's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NewSessionMark {
    Done,
    /// Until `until`, in unix seconds on the daemon's clock, saved as
    /// given: one already past is a snooze that has ended.
    Snoozed {
        until: u64,
    },
    Pinned,
    Kept,
}

/// Params of [`SESSION_MARK`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionMarkParams {
    pub session: SessionId,
    /// The mark to save in place of the session's own; absent clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<NewSessionMark>,
}

impl SessionMarkParams {
    /// Builds it from each of its fields.
    pub fn new(session: SessionId, mark: Option<NewSessionMark>) -> Self {
        Self { session, mark }
    }
}

/// Result of [`SESSION_MARK`]: the mark is saved, on a session the daemon
/// has not loaded too, and clients attached to the session receive it as
/// [`super::SessionEvent::MarkChanged`] if it changed. The session's
/// [`super::SessionSummary::updated_at`] is as it was. A session nothing
/// is saved under is [`crate::ErrorCode::SessionNotFound`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionMarkResponse {
    /// The session's mark as saved, with the time the daemon stamped;
    /// absent once cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<SessionMark>,
}

impl SessionMarkResponse {
    /// Builds it from each of its fields.
    pub fn new(mark: Option<SessionMark>) -> Self {
        Self { mark }
    }
}

#[cfg(test)]
#[path = "mark_tests.rs"]
mod tests;
