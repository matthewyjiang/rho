use serde::{Deserialize, Serialize};

/// Which transcripts a search or read may see. Every scope except `Current`
/// excludes the current session, whose recent turns are usually still in context.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Scope {
    #[default]
    Repo,
    Worktree,
    All,
    /// Only the current session, e.g. to recover turns compaction summarized away.
    Current,
}
