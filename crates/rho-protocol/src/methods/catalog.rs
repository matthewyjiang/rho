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
/// Set a saved session's title.
pub const SESSION_RENAME: &str = "rho/session/rename";
/// Delete a saved session, with the sessions it started.
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
}

/// Result of [`SESSION_DELETE`]: the session's files are gone, with those
/// of the sessions it started, such as its subagents', and the daemon has
/// unloaded it, detaching its clients as
/// [`super::DetachReason::Deleted`]. Its job stays in [`super::JOB_LIST`].
///
/// A session whose turn is under way, a settings change applying,
/// background work running, such as a subagent, or a run it started not yet
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
