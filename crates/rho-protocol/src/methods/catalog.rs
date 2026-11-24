//! The session catalog: the saved sessions a person can resume, as a
//! frontend lists, renames, and deletes them, so ACP's `session/list` and
//! `session/delete` (#1670) and the TUI's sessions hub (#1667) stop reading
//! session files themselves. Resuming one is [`super::SESSION_ATTACH`],
//! which reopens a saved session the daemon has not loaded.

use std::{num::NonZeroU32, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

/// List the sessions a person can resume, newest first, a page at a time.
pub const SESSION_LIST: &str = "rho/session/list";
/// List the sessions [`SESSION_LIST`] lists, with the same params and
/// paging, each with the agent it runs as and the session that started it:
/// what a client that groups sessions under the one that started them
/// reads. A daemon from before this method answers
/// [`crate::ErrorCode::MethodNotFound`].
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): fold `agent` and `parent` into SessionSummary and remove rho/session/list_detailed (SESSION_LIST_DETAILED, SessionListDetailedResponse, DetailedSessionSummary).
///
/// [`SessionSummary`] is not `#[non_exhaustive]`, so a field is a major
/// change; until then this method sends its rows with those fields beside
/// them.
pub const SESSION_LIST_DETAILED: &str = "rho/session/list_detailed";
/// Set a saved session's title.
pub const SESSION_RENAME: &str = "rho/session/rename";
/// Delete a saved session, with its subagents', workflow nodes', and forks'
/// sessions. Sessions it started through session control stay.
pub const SESSION_DELETE: &str = "rho/session/delete";

/// Params of [`SESSION_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionListParams {
    /// Only the sessions saved in this workspace, an absolute path,
    /// resolved through symlinks as [`super::SESSION_NEW`] resolves its
    /// `cwd`; absent lists every workspace's. A path that does not resolve
    /// to a directory lists none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The [`SessionListResponse::next_cursor`] of the page before; absent
    /// for the first page. A cursor the daemon did not issue is refused as
    /// [`crate::ErrorCode::InvalidParams`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The most sessions the page holds: 100 when absent, and at most the
    /// daemon's limit, 1000, which the error refusing a larger one names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<NonZeroU32>,
}

/// Result of [`SESSION_LIST`]. Following `next_cursor` never lists a session
/// twice, nor skips one when others are deleted between pages; a session
/// saved during the walk moves ahead of the cursor and is missed, so list
/// again from the first page for a fresh view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionSummary>,
    /// The cursor of the next page; absent on the last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// One session, as [`SESSION_LIST`] reports it. Only sessions with a saved
/// prompt whose workspace still exists are listed, and not those a program
/// ran: `rho run`'s, subagents', and workflow nodes'.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummary {
    pub session: SessionId,
    /// The workspace the session was saved in, which an attach reopens it
    /// in.
    pub cwd: PathBuf,
    /// The directories the session added beside its workspace, as last
    /// saved, those since removed left out.
    pub added_dirs: Vec<PathBuf>,
    /// The session's title; absent until it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The session's first user message on one line, cut to 80 characters,
    /// which an untitled session is labelled with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_user_message: Option<String>,
    /// The session's last user message, cut like `first_user_message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message: Option<String>,
    /// Unix time, in seconds, of the session's last save.
    pub updated_at: u64,
}

/// Result of [`SESSION_LIST_DETAILED`]: a page of [`SESSION_LIST`]'s, its
/// rows detailed.
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): fold `agent` and `parent` into SessionSummary and remove rho/session/list_detailed (SESSION_LIST_DETAILED, SessionListDetailedResponse, DetailedSessionSummary).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionListDetailedResponse {
    pub sessions: Vec<DetailedSessionSummary>,
    /// The cursor of the next page, as [`SessionListResponse::next_cursor`];
    /// absent on the last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl SessionListDetailedResponse {
    /// Builds it from each of its fields.
    pub fn new(sessions: Vec<DetailedSessionSummary>, next_cursor: Option<String>) -> Self {
        Self {
            sessions,
            next_cursor,
        }
    }
}

/// One session, as [`SESSION_LIST_DETAILED`] reports it: its
/// [`SessionSummary`]'s fields, then the agent it runs as and the session
/// that started it.
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): fold `agent` and `parent` into SessionSummary and remove rho/session/list_detailed (SESSION_LIST_DETAILED, SessionListDetailedResponse, DetailedSessionSummary).
///
/// On the wire it is a [`SessionSummary`] with two more optional fields, so
/// folding them into it changes no row a client reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct DetailedSessionSummary {
    #[serde(flatten)]
    pub summary: SessionSummary,
    /// The agent the session runs as, by id, as
    /// [`super::SessionNewParams::agent`] takes it: the one a `/agent:` first
    /// prompt rebound it to, if one did. Absent for the default agent, and
    /// for a session saved before agents were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The session that started this one through session control, as
    /// [`super::SessionNewParams::parent`] named it, saved with the session.
    /// Absent for a session nothing started that way, or one started before
    /// the daemon saved it. Deleting the parent leaves this session listed,
    /// still naming it: an id [`SESSION_LIST`] no longer lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
}

impl DetailedSessionSummary {
    /// Builds it from each of its fields.
    pub fn new(summary: SessionSummary, agent: Option<String>, parent: Option<SessionId>) -> Self {
        Self {
            summary,
            agent,
            parent,
        }
    }
}

/// Params of [`SESSION_RENAME`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRenameParams {
    pub session: SessionId,
    /// The new title, surrounding whitespace trimmed. One that is empty,
    /// holds a control character such as a newline, or runs over the
    /// daemon's 500 characters is refused as
    /// [`crate::ErrorCode::InvalidParams`], naming why.
    pub title: String,
}

/// Result of [`SESSION_RENAME`]: the title is saved, and clients attached
/// to the session receive it as [`super::SessionEvent::TitleChanged`]. A
/// generated title never replaces it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRenameResponse {}

/// Params of [`SESSION_DELETE`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDeleteParams {
    pub session: SessionId,
    /// Refuse a session a client is attached to, as
    /// [`crate::ErrorCode::SessionInUse`], rather than detach its clients:
    /// for a client that deletes sessions it is not showing and cannot
    /// tell which of them another client has open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unless_attached: bool,
}

/// Result of [`SESSION_DELETE`]: the session's files are gone, with those
/// of the sessions it started, such as its subagents', and the daemon has
/// unloaded it, detaching its clients as
/// [`super::DetachReason::Deleted`]. Its job stays in [`super::JOB_LIST`].
/// The sessions it started through session control
/// ([`super::SessionNewParams::parent`]) stay, still naming it as
/// [`DetailedSessionSummary::parent`].
///
/// A session whose turn is under way, a settings change waiting or
/// applying, background work running, such as a subagent, or a run it started not yet
/// finished is refused as [`crate::ErrorCode::SessionBusy`]; one another
/// process has open, or one a session it started is open in, as
/// [`crate::ErrorCode::SessionInUse`]. Either refusal leaves a loaded
/// session loaded. A session the daemon loaded but that has saved nothing
/// yet is unloaded all the same. An id the daemon has not loaded and nothing
/// is saved under is [`crate::ErrorCode::SessionNotFound`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDeleteResponse {}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
