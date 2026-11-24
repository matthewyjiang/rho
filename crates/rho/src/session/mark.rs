//! A session's mark: how a person has organised it among their sessions
//! (done, snoozed, pinned, or put back), which the daemon keeps so that
//! every frontend shows the same list (`rho/session/mark`).
//!
//! The mark is the `mark` column of the session's index row, beside its
//! title: neither can be rebuilt from the transcript, so each write is made
//! durable, and a re-summary of the transcript keeps both. A session's
//! `updated_at` is its newest transcript entry's time, so a mark, which
//! writes no entry, never moves it; clients lean on that to tell a session
//! marked done from one that was active since. Deleting a session removes
//! its row and its mark with it, and a fork is a new session with a row of
//! its own, so it starts with none.

use std::path::Path;

use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{
    index::{open_index, sync_committed_writes},
    persistence::{session_root_in, unix_timestamp_secs, workspace_key, SessionStore},
    NoSessionFound, Session,
};

/// A session's mark. Every time is unix seconds on the clock that stamps
/// the session's saves, so it compares with `SessionSummary::updated_at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum SessionMark {
    /// Finished with, as of `at`.
    Done { at: u64 },
    /// Hidden until `until`, which the person chose.
    Snoozed { until: u64 },
    /// Kept on top since `at`.
    Pinned { at: u64 },
    /// Put back on the active list at `at`.
    Kept { at: u64 },
}

impl SessionMark {
    /// Reads the `mark` column. A mark this Rho cannot read, as one a later
    /// Rho saved, reads as none, so the session stays listed.
    pub(super) fn decode(column: Option<String>) -> Option<Self> {
        serde_json::from_str(&column?).ok()
    }
}

/// A mark a person asks for, before the store stamps the time it is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewMark {
    Done,
    Snoozed { until: u64 },
    Pinned,
    Kept,
}

impl NewMark {
    /// The mark as saved at `now`: only a snooze's end is the person's own.
    pub(crate) fn at(self, now: u64) -> SessionMark {
        match self {
            Self::Done => SessionMark::Done { at: now },
            Self::Snoozed { until } => SessionMark::Snoozed { until },
            Self::Pinned => SessionMark::Pinned { at: now },
            Self::Kept => SessionMark::Kept { at: now },
        }
    }
}

/// A session's mark before and after a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MarkChange {
    pub(crate) before: Option<SessionMark>,
    pub(crate) after: Option<SessionMark>,
}

impl Session {
    /// Replaces this session's mark with what `change` makes of the one it
    /// has and of the time now, on the clock that stamps its saves. The
    /// session's `updated_at` stays as it was.
    pub(crate) fn change_mark(
        &self,
        change: impl FnOnce(Option<SessionMark>, u64) -> Option<SessionMark>,
    ) -> anyhow::Result<MarkChange> {
        change_mark(&self.session_root, &self.workspace_key, &self.id, change)
    }

    /// [`Self::change_mark`] of the session saved under exactly `id`, in
    /// whichever workspace, among the sessions `rho_home` keeps, open or
    /// not: `None` when nothing is saved under it.
    pub(crate) fn change_mark_of(
        rho_home: &Path,
        id: &str,
        change: impl FnOnce(Option<SessionMark>, u64) -> Option<SessionMark>,
    ) -> anyhow::Result<Option<MarkChange>> {
        let root = session_root_in(rho_home);
        let Some(cwd) = SessionStore::workspace_of(&root, id)? else {
            return Ok(None);
        };
        match change_mark(&root, &workspace_key(&cwd), id, change) {
            Ok(change) => Ok(Some(change)),
            // Deleted since the lookup.
            Err(error) if error.is::<NoSessionFound>() => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// Reads and replaces the mark of the row `workspace_key` and `id` name in
/// one write transaction, so two changes at once, from this process or
/// another, each see the other's mark or come before it.
fn change_mark(
    session_root: &Path,
    workspace_key: &str,
    id: &str,
    change: impl FnOnce(Option<SessionMark>, u64) -> Option<SessionMark>,
) -> anyhow::Result<MarkChange> {
    let connection = open_index(session_root)?;
    let mut connection = connection
        .lock()
        .expect("session index connection poisoned");
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let column = transaction
        .query_row(
            "select mark from sessions where workspace_key = ?1 and id = ?2",
            params![workspace_key, id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .ok_or_else(|| NoSessionFound {
            id_prefix: id.to_owned(),
        })?;
    let before = SessionMark::decode(column.clone());
    let after = change(before, unix_timestamp_secs());
    let saved = after.map(|mark| serde_json::to_string(&mark)).transpose()?;
    // Compared as saved, so clearing a mark this Rho cannot read removes it.
    let written = saved != column;
    if written {
        transaction.execute(
            "update sessions set mark = ?3 where workspace_key = ?1 and id = ?2",
            params![workspace_key, id, saved],
        )?;
    }
    transaction.commit()?;
    // Also when nothing was written, so asking again for a mark whose flush
    // failed flushes it. The mark is already visible; say so rather than
    // imply it was lost.
    sync_committed_writes(connection)
        .map_err(|error| error.context("mark saved but could not be flushed to disk"))?;
    Ok(MarkChange { before, after })
}

#[cfg(test)]
#[path = "mark_tests.rs"]
mod tests;
