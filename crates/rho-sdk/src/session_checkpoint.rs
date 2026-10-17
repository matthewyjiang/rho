//! Incremental commits for a run's step checkpoints.
//!
//! Between a run's commits its working history only grows. Compaction, the
//! one in-place rewrite, commits its replacement and so moves the revision.
//! While the session still holds the run's last commit, the committed history
//! is therefore a prefix of the working history, and a checkpoint clones only
//! the messages appended since. Context accounting relies on the same
//! invariant: it advances by message count from its last measured length.

use crate::{model::Message, Error, Revision};

use super::{ContextCommit, SessionCore, SessionData};

/// A committed revision and its history length, minted only by
/// [`SessionCore`]. A run keeps its last mark so the next checkpoint can tell
/// whether the session still holds a prefix of the working history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommitMark {
    revision: Revision,
    history_len: usize,
}

impl CommitMark {
    pub(crate) fn revision(self) -> Revision {
        self.revision
    }
}

impl SessionData {
    fn commit_mark(&self) -> CommitMark {
        CommitMark {
            revision: self.revision,
            history_len: self.history.len(),
        }
    }

    /// Whether the session holds commit `mark` and it is `history`'s prefix.
    ///
    /// Release builds compare only the last committed message, which costs
    /// one message instead of the history and catches a replaced tail; debug
    /// builds check the whole prefix.
    fn holds_prefix(&self, mark: CommitMark, history: &[Message]) -> bool {
        let Some(prefix) = history.get(..mark.history_len) else {
            return false;
        };
        let holds = self.commit_mark() == mark && prefix.last() == self.history.last();
        debug_assert!(
            !holds || self.history == prefix,
            "working history was rewritten without a commit"
        );
        holds
    }
}

impl SessionCore {
    /// Copies the committed history into a run's working history, with the
    /// mark its first checkpoint extends.
    pub(crate) fn snapshot_for_run(&self) -> (Vec<Message>, CommitMark) {
        let data = self.lock_data();
        (data.history.clone(), data.commit_mark())
    }

    /// Whether the session still holds exactly the commit `mark` and
    /// `history` has not grown since.
    pub(crate) fn holds_unchanged(&self, mark: CommitMark, history: &[Message]) -> bool {
        history.len() == mark.history_len && self.lock_data().holds_prefix(mark, history)
    }

    /// Commits a run's working history at a step checkpoint, cloning only the
    /// messages appended since `previous` while the session still holds it.
    /// Otherwise the whole history is copied.
    pub(crate) fn commit_checkpoint(
        &self,
        history: &[Message],
        previous: CommitMark,
    ) -> Result<CommitMark, Error> {
        // Accounting updates only context fields, so `data` still holds the
        // previous commit here.
        let (mut data, revision) = self.begin_commit(history, ContextCommit::Append)?;
        if data.holds_prefix(previous, history) {
            data.history
                .extend_from_slice(&history[previous.history_len..]);
        } else {
            data.history = history.to_vec();
        }
        data.revision = revision;
        Ok(data.commit_mark())
    }
}
