//! The session catalog: the saved sessions a person can resume, as a
//! frontend lists, renames, and deletes them, so ACP's `session/list` and
//! `session/delete` (#1670) and the TUI's sessions hub (#1667) stop reading
//! session files themselves. Resuming one is [`super::SESSION_ATTACH`],
//! which reopens a saved session the daemon has not loaded.

use std::{num::NonZeroU32, ops::Range, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{MessageId, SessionId, SessionMark, ToolCallId};

/// List the sessions a person can resume, newest first, a page at a time.
pub const SESSION_LIST: &str = "rho/session/list";
/// List the sessions [`SESSION_LIST`] lists, with the same params and
/// paging, each with the agent it runs as, the session that started it,
/// and its mark: what a client that groups sessions under the one that
/// started them, or sorts them by their marks, reads. A daemon from before
/// this method answers [`crate::ErrorCode::MethodNotFound`].
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): fold agent, parent and mark into SessionSummary and remove rho/session/list_detailed.
///
/// [`SessionSummary`] is not `#[non_exhaustive]`, so a field is a major
/// change; until then this method sends its rows with those fields beside
/// them.
pub const SESSION_LIST_DETAILED: &str = "rho/session/list_detailed";
/// Find the saved sessions whose conversation holds every word of a
/// query, newest first, a page at a time. Searches the sessions
/// [`SESSION_LIST`] lists, in the branch of each that an attach replays. A
/// daemon from before this method answers
/// [`crate::ErrorCode::MethodNotFound`].
pub const SESSION_SEARCH: &str = "rho/session/search";
/// Set a saved session's title.
pub const SESSION_RENAME: &str = "rho/session/rename";
/// Delete a saved session, with its subagents' and forks' sessions. Sessions it started through session control stay.
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
/// rows detailed. Goes at the next major with [`SESSION_LIST_DETAILED`].
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
/// [`SessionSummary`]'s fields, then the agent it runs as, the session that
/// started it, and its mark. Goes at the next major with
/// [`SESSION_LIST_DETAILED`].
///
/// On the wire it is a [`SessionSummary`] with three more optional fields,
/// so folding them into it changes no row a client reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct DetailedSessionSummary {
    #[serde(flatten)]
    pub summary: SessionSummary,
    /// The agent the session runs as, by id, as
    /// [`super::SessionNewParams::agent`] takes it: the one a `/agent:` first
    /// prompt rebound it to, if one did, and `default` for the default
    /// agent. Absent for a session saved before agents were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The session that started this one through session control, saved
    /// with the session: the one [`super::SessionNewParams::parent`] named,
    /// or, when that names a subagent's or workflow node's session, the
    /// session whose turn runs that subagent or workflow. Absent for a
    /// session nothing started that way, one whose `parent` no job of the
    /// daemon writes, and one started before the daemon saved it. The id
    /// alone names the parent, and a list may not hold it: the parent may be
    /// deleted, a fork, unsaved, in another workspace or on another page.
    /// A client treats a parent it cannot find as none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// How the person has marked the session ([`super::SESSION_MARK`]);
    /// absent while it has no mark, and from a daemon that keeps none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<SessionMark>,
}

impl DetailedSessionSummary {
    /// Builds it from each of its fields.
    pub fn new(
        summary: SessionSummary,
        agent: Option<String>,
        parent: Option<SessionId>,
        mark: Option<SessionMark>,
    ) -> Self {
        Self {
            summary,
            agent,
            parent,
            mark,
        }
    }
}

/// Params of [`SESSION_SEARCH`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionSearchParams {
    /// Words a message must all hold, ignoring case and stemmed as the
    /// index stems them; the last also matches as a prefix, so results
    /// follow typing. Literal: no operators. Blank is
    /// [`crate::ErrorCode::InvalidParams`], and so is one over the daemon's
    /// limit, which the error names.
    pub query: String,
    /// Only the sessions saved in this workspace, as
    /// [`SessionListParams::cwd`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The [`SessionSearchResponse::next_cursor`] of the page before;
    /// absent for the first page. A cursor the daemon did not issue is
    /// [`crate::ErrorCode::InvalidParams`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Sessions per page: 20 when absent, and at most the daemon's limit,
    /// 100, which the error refusing a larger one names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<NonZeroU32>,
}

impl SessionSearchParams {
    /// Builds it from each of its fields.
    pub fn new(
        query: impl Into<String>,
        cwd: Option<PathBuf>,
        cursor: Option<String>,
        limit: Option<NonZeroU32>,
    ) -> Self {
        Self {
            query: query.into(),
            cwd,
            cursor,
            limit,
        }
    }
}

/// Result of [`SESSION_SEARCH`]: the matching sessions, newest
/// `updated_at` first, the more relevant first among those saved in the
/// same second. Following `next_cursor` pages as [`SESSION_LIST`] does: a
/// session saved during the walk moves ahead of the cursor and is missed.
/// Relevance moves as the index changes, so a session saved in the same
/// second as the last one a page returned may be repeated or missed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionSearchResponse {
    pub sessions: Vec<SessionSearchHit>,
    /// The cursor of the next page; absent on the last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// How many sessions match in all.
    pub total: u64,
    /// The index has not read every transcript yet (first use, after its
    /// schema changed, or for a few searches while one whose read failed
    /// or that changed as it was read waits to be read again), so this
    /// page may miss matches. Search again later.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub indexing: bool,
}

impl SessionSearchResponse {
    /// Builds it from each of its fields.
    pub fn new(
        sessions: Vec<SessionSearchHit>,
        next_cursor: Option<String>,
        total: u64,
        indexing: bool,
    ) -> Self {
        Self {
            sessions,
            next_cursor,
            total,
            indexing,
        }
    }
}

/// One session [`SESSION_SEARCH`] found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SessionSearchHit {
    /// The session as [`SESSION_LIST_DETAILED`] shows it, so a client
    /// needn't have listed it.
    pub session: DetailedSessionSummary,
    /// How many of its messages match.
    pub matching_messages: u64,
    /// Its best one or two matching messages, best first.
    pub excerpts: Vec<SearchExcerpt>,
}

impl SessionSearchHit {
    /// Builds it from each of its fields.
    pub fn new(
        session: DetailedSessionSummary,
        matching_messages: u64,
        excerpts: Vec<SearchExcerpt>,
    ) -> Self {
        Self {
            session,
            matching_messages,
            excerpts,
        }
    }
}

/// A message [`SESSION_SEARCH`] matched, around its first match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct SearchExcerpt {
    pub role: ExcerptRole,
    /// At most ~320 characters of the message, around its first match.
    pub text: String,
    /// The matched words in `text`, as byte ranges on char boundaries, in
    /// order (FTS5 `highlight()`).
    pub highlights: Vec<Range<u32>>,
    /// Where an attach's replay shows the message, by the id it gives it;
    /// absent when no replayed event shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<SearchLocation>,
    /// When the message was saved, unix seconds (the clock of
    /// [`SessionSummary::updated_at`]); absent when its time doesn't read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<u64>,
}

impl SearchExcerpt {
    /// Builds it from each of its fields.
    pub fn new(
        role: ExcerptRole,
        text: impl Into<String>,
        highlights: Vec<Range<u32>>,
        at: Option<SearchLocation>,
        saved_at: Option<u64>,
    ) -> Self {
        Self {
            role,
            text: text.into(),
            highlights,
            at,
            saved_at,
        }
    }
}

/// Who wrote a [`SearchExcerpt`]'s message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExcerptRole {
    /// The person, or a host input a replay shows as theirs.
    User,
    /// The agent: its text and the tool calls it wrote.
    Agent,
    /// A tool's output.
    ToolResult,
    /// A tool's output when the call failed.
    ToolError,
    /// A role a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// The replayed event that shows a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SearchLocation {
    /// A user or agent message: the [`MessageId`] its replay's first event
    /// carries, which its live events carried when a turn streamed it.
    Message { id: MessageId },
    /// A tool result, or an agent message that only calls tools: the
    /// [`ToolCallId`] of the call replay opens for it.
    ToolCall { id: ToolCallId },
    /// A location a newer daemon sends that this crate does not know.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
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
/// of the sessions it owns, its subagents' and forks',
/// and the daemon has unloaded it, detaching its clients as
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
