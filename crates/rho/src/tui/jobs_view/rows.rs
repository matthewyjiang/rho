//! Which of the daemon's jobs the jobs view lists, in what order, and what
//! each row says. Pure: the view draws these and acts on the job a row
//! names.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use rho_protocol::{JobId, JobKind, JobState, JobSummary};

use crate::tui::{elapsed::format_elapsed, session_picker::short_session_id, usage_cost};

/// One project's jobs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Group {
    pub(super) workspace: PathBuf,
    pub(super) rows: Vec<Row>,
}

/// One listed job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub(super) job: JobId,
    /// How many listed jobs lead to it: 0 for a job a client started.
    pub(super) depth: usize,
    pub(super) status: Status,
    pub(super) title: String,
    /// The branch of the worktree the job's session works in, if it does,
    /// and whether that worktree is stale, then the model, turn time, and
    /// spend the daemon follows, those it does.
    pub(super) facts: Vec<String>,
}

/// What a job is doing, as its row says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    NeedsInput,
    Running,
    Queued,
    Idle,
    Ended,
}

impl Status {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NeedsInput => "needs input",
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Idle => "idle",
            Self::Ended => "ended",
        }
    }
}

/// Whether `job` waits on a person: its own turn's request, or a question
/// one of its subagents asked it.
pub(super) fn needs_input(job: &JobSummary) -> bool {
    job.state == JobState::WaitingForInput
        || job
            .live
            .as_ref()
            .is_some_and(|live| !live.inputs.is_empty())
}

/// Whether the view lists `job`: one under way, one the daemon follows,
/// which a loaded session's is even between turns, or one whose stale
/// worktree waits on a person to merge or remove it.
fn listed(job: &JobSummary) -> bool {
    job.state.in_progress() || job.live.is_some() || job.worktree_stale
}

/// The listed jobs among `jobs` that match `filter`, by project, each
/// project's jobs oldest first with the runs a job started under it.
/// `jobs` is oldest first, as the daemon lists them. A session in a
/// worktree lists under the project the worktree was added from, and every
/// run under the project of the job it nests under.
pub(super) fn groups(jobs: &[JobSummary], filter: &str) -> Vec<Group> {
    let shown: Vec<&JobSummary> = jobs.iter().filter(|job| listed(job)).collect();
    let ids: HashSet<&JobId> = shown.iter().map(|job| &job.id).collect();
    let needle = filter.trim().to_lowercase();
    let mut by_workspace: BTreeMap<&Path, Vec<Row>> = BTreeMap::new();
    // A job whose parent is not listed, such as a run of a session the
    // daemon unloaded, lists as a root.
    let roots = shown.iter().filter(|job| {
        job.parent
            .as_ref()
            .is_none_or(|parent| !ids.contains(parent))
    });
    for root in roots {
        let project = root.project();
        let mut stack = vec![(*root, 0)];
        while let Some((job, depth)) = stack.pop() {
            let row = row(job, depth);
            if matches(&row, job, &needle) {
                by_workspace.entry(project).or_default().push(row);
            }
            let children = shown
                .iter()
                .filter(|child| child.parent.as_ref() == Some(&job.id));
            // Reversed, so the oldest child comes off the stack first.
            stack.extend(children.rev().map(|child| (*child, depth + 1)));
        }
    }
    by_workspace
        .into_iter()
        .map(|(workspace, rows)| Group {
            workspace: workspace.to_path_buf(),
            rows,
        })
        .collect()
}

fn matches(row: &Row, job: &JobSummary, needle: &str) -> bool {
    needle.is_empty()
        || [
            row.title.as_str(),
            row.status.label(),
            &job.workspace.to_string_lossy(),
        ]
        .into_iter()
        .chain(row.facts.iter().map(String::as_str))
        .any(|text| text.to_lowercase().contains(needle))
}

fn row(job: &JobSummary, depth: usize) -> Row {
    let live = job.live.as_ref();
    let mut facts: Vec<String> = job
        .worktree
        .iter()
        .map(|worktree| worktree.branch.clone())
        .collect();
    if job.worktree_stale {
        facts.push("stale worktree".to_owned());
    }
    if let Some(live) = live {
        facts.push(live.model.model.clone());
        if let Some(elapsed) = live.turn_elapsed_ms {
            facts.push(format_elapsed(Duration::from_millis(elapsed)));
        }
        if let Some(cost) = live.usage.and_then(|usage| usage.cost_usd_micros) {
            facts.push(usage_cost::format_usd(cost));
        }
    }
    Row {
        job: job.id.clone(),
        depth,
        status: status(job),
        title: live
            .and_then(|live| live.title.clone())
            .unwrap_or_else(|| untitled(job)),
        facts,
    }
}

fn status(job: &JobSummary) -> Status {
    if needs_input(job) {
        return Status::NeedsInput;
    }
    match job.state {
        JobState::Running | JobState::WaitingForInput => Status::Running,
        JobState::Queued => Status::Queued,
        JobState::Idle => Status::Idle,
        JobState::Completed
        | JobState::Failed
        | JobState::Cancelled
        | JobState::TimedOut
        | JobState::Interrupted => Status::Ended,
        // `JobState` is non-exhaustive; the daemon follows a job it lists
        // with a state this rho does not know.
        _ => Status::Running,
    }
}

/// What a job with no title yet is called: its kind and the id a person
/// names it by.
fn untitled(job: &JobSummary) -> String {
    let named = |kind: &str, id: Option<&str>| match id {
        Some(id) => format!("{kind} {}", short_session_id(id)),
        None => format!("{kind} {}", short_session_id(job.id.as_str())),
    };
    match job.kind {
        JobKind::Session => named("session", job.session.as_ref().map(|id| id.as_str())),
        JobKind::Subagent => named("subagent", job.run.as_deref()),
        JobKind::Workflow => named("workflow", job.workflow_run.as_deref()),
        // `JobKind` is non-exhaustive.
        _ => named("job", None),
    }
}

#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;
