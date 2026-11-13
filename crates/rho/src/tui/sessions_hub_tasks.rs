//! Background work for session management: listing every saved session and
//! deleting sessions run on the blocking pool, so the event loop keeps drawing
//! and taking input while transcripts are scanned or removed.
//!
//! One task runs at a time. A delete also reloads the listing of the picker
//! that started it on the same worker, so the refresh never waits on disk.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use tokio::task::{JoinError, JoinHandle};

use super::sessions_hub_groups::{hub_groups, HubGroup, Placement};
use super::{session_picker, statusline::path::compact_cwd, App, ComposerMode, Entry, Session};
use crate::session::{
    is_in_use, CleanupFailure, CleanupOutcome, DeleteOptions, DeleteOutcome, SessionListing,
    SessionSummary, SessionTarget, Workspace, WorkspaceDeleteOutcome,
};

/// Deletes sessions the store refused because something holds them open,
/// through the daemon, which holds every session it has loaded and lets go
/// of one no client is attached to: one answer per id, in order, a refusal
/// in the daemon's words. Blocking.
pub(super) type DeleteHeld = Box<dyn FnOnce(Vec<String>) -> Vec<Result<(), String>> + Send>;

/// Sessions a confirmed delete removes.
#[derive(Clone, Debug)]
pub(super) enum SessionsDelete {
    One(SessionTarget),
    Directory {
        cwd: PathBuf,
        targets: Vec<SessionTarget>,
    },
    /// Sessions whose workspace directory no longer exists.
    CleanupMissing(Vec<SessionTarget>),
}

impl SessionsDelete {
    fn session_count(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Directory { targets, .. } | Self::CleanupMissing(targets) => targets.len(),
        }
    }

    /// Deletes from the session store, in one batch, then through `held`
    /// what the store refused as open elsewhere.
    fn run(self, options: DeleteOptions, held: DeleteHeld) -> DeleteReport {
        match self {
            Self::One(target) => {
                let result = match Session::delete_target(&target, options) {
                    Ok(outcome) => Ok(Deleted::from(&outcome)),
                    Err(error) if is_in_use(&error) => {
                        match held(vec![target.id.clone()]).pop() {
                            // The daemon does not count what went with it.
                            Some(Ok(())) => Ok(Deleted::default()),
                            Some(Err(refusal)) => Err(anyhow::anyhow!(refusal)),
                            None => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                DeleteReport::One { target, result }
            }
            Self::Directory { cwd, targets } => DeleteReport::Directory {
                result: Session::delete_targets(&targets, options)
                    .map(|outcome| BatchDeleted::from(outcome).retry_held(held)),
                cwd,
            },
            Self::CleanupMissing(targets) => DeleteReport::CleanupMissing(
                Session::cleanup_missing_targets(&targets, options)
                    .map(|outcome| BatchDeleted::from(outcome).retry_held(held)),
            ),
        }
    }
}

/// What went with one deleted session, as its notice counts it.
#[derive(Debug, Default)]
pub(super) struct Deleted {
    child_sessions: usize,
    runs: usize,
}

impl From<&DeleteOutcome> for Deleted {
    fn from(outcome: &DeleteOutcome) -> Self {
        Self {
            child_sessions: outcome.deleted_child_session_count,
            runs: outcome.deleted_run_count,
        }
    }
}

/// What a delete of many sessions did, as its notice counts it.
#[derive(Debug, Default)]
pub(super) struct BatchDeleted {
    deleted: usize,
    failures: Vec<CleanupFailure>,
    kept_current: bool,
    /// Sessions skipped because their workspace directory came back.
    restored_workspaces: usize,
}

impl From<WorkspaceDeleteOutcome> for BatchDeleted {
    fn from(outcome: WorkspaceDeleteOutcome) -> Self {
        Self {
            deleted: outcome.deleted.len(),
            failures: outcome.failures,
            kept_current: !outcome.kept_protected.is_empty(),
            restored_workspaces: 0,
        }
    }
}

impl From<CleanupOutcome> for BatchDeleted {
    fn from(outcome: CleanupOutcome) -> Self {
        Self {
            deleted: outcome.deleted.len(),
            failures: outcome.failures,
            kept_current: false,
            restored_workspaces: outcome.restored_workspaces,
        }
    }
}

impl BatchDeleted {
    /// Asks `held` for the sessions the store refused as open elsewhere:
    /// each it deletes counts as deleted, and each it refuses keeps the
    /// failure, in its words.
    fn retry_held(mut self, held: DeleteHeld) -> Self {
        let (in_use, mut failures): (Vec<_>, Vec<_>) = std::mem::take(&mut self.failures)
            .into_iter()
            .partition(|failure| failure.in_use);
        if !in_use.is_empty() {
            let mut answers =
                held(in_use.iter().map(|failure| failure.id.clone()).collect()).into_iter();
            for mut failure in in_use {
                match answers.next() {
                    Some(Ok(())) => self.deleted += 1,
                    Some(Err(refusal)) => {
                        failure.error = refusal;
                        failures.push(failure);
                    }
                    // Unanswered: the store's refusal stands.
                    None => failures.push(failure),
                }
            }
        }
        self.failures = failures;
        self
    }
}

/// Picker a delete returns to, which decides the listing to reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeleteOrigin {
    /// The `/sessions` hub: reload every workspace.
    Hub,
    /// The `/resume` picker: reload the current workspace only.
    Resume,
}

#[derive(Debug)]
pub(super) enum DeleteReport {
    One {
        target: SessionTarget,
        result: anyhow::Result<Deleted>,
    },
    Directory {
        cwd: PathBuf,
        result: anyhow::Result<BatchDeleted>,
    },
    CleanupMissing(anyhow::Result<BatchDeleted>),
}

/// Listing reloaded after a delete, for the picker that started it.
#[derive(Debug)]
pub(super) enum Relisted {
    Hub(anyhow::Result<Vec<HubGroup>>),
    Resume(anyhow::Result<Vec<SessionSummary>>),
}

#[derive(Debug)]
pub(super) enum TaskOutput {
    Load(anyhow::Result<Vec<HubGroup>>),
    Delete {
        report: DeleteReport,
        relisted: Relisted,
    },
}

/// The one in-flight sessions task.
#[derive(Debug)]
pub(super) struct PendingSessionsTask {
    handle: JoinHandle<TaskOutput>,
    kind: TaskKind,
}

/// A load is a read and can be superseded; a delete must finish first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskKind {
    Load,
    Delete,
}

/// Resolves when the pending task finishes, so the event loop wakes at once
/// instead of on its next idle tick. Pends forever when no task runs.
pub(super) async fn next_sessions_task(
    pending: &mut Option<PendingSessionsTask>,
) -> Result<TaskOutput, JoinError> {
    let Some(task) = pending.as_mut() else {
        return std::future::pending().await;
    };
    let output = (&mut task.handle).await;
    *pending = None;
    output
}

/// Lists and groups every saved session (see [`hub_groups`] for which it
/// shows). Blocking: reconciles the session index and stats every
/// workspace directory.
fn load_hub_groups(current_cwd: &Path) -> anyhow::Result<Vec<HubGroup>> {
    let sessions = Session::list_all(SessionListing::Everything)?;
    let mut missing_directories = HashSet::new();
    for cwd in sessions.iter().map(|session| &session.cwd) {
        if !missing_directories.contains(cwd) && Session::workspace_directory_is_missing(cwd)? {
            missing_directories.insert(cwd.clone());
        }
    }
    // Resolving a deleted directory would walk its surviving ancestors into an
    // unrelated repository, so missing directories never resolve.
    Ok(hub_groups(sessions, current_cwd, |cwd| {
        if missing_directories.contains(cwd) {
            return Placement::Missing;
        }
        let workspace = Workspace::resolve(cwd);
        if workspace.is_git() {
            Placement::Repo(workspace)
        } else {
            Placement::Lone
        }
    }))
}

/// `1 session`, `3 sessions`.
pub(super) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

impl App {
    /// While a delete runs, report it and return true. Other deletes,
    /// resumes, and reloads wait so none acts on sessions it is removing.
    pub(super) fn refuse_while_sessions_delete_runs(&mut self) -> bool {
        let running = self
            .pending_sessions_task
            .as_ref()
            .is_some_and(|task| task.kind == TaskKind::Delete);
        if running {
            self.set_status("sessions are still being deleted");
        }
        running
    }

    /// Start listing every session; [`Self::fill_sessions_hub`] shows it.
    /// Replaces a pending load, whose result nothing waits for anymore; the
    /// blocking read runs to completion and is dropped.
    pub(super) fn start_sessions_load(&mut self) {
        let current_cwd = self.info.runtime.cwd.clone();
        self.pending_sessions_task = Some(PendingSessionsTask {
            handle: tokio::task::spawn_blocking(move || {
                TaskOutput::Load(load_hub_groups(&current_cwd))
            }),
            kind: TaskKind::Load,
        });
    }

    /// Delete `request` in the background. The picker stays usable and
    /// refreshes when the delete lands.
    pub(super) fn start_sessions_delete(&mut self, request: SessionsDelete, origin: DeleteOrigin) {
        let options = DeleteOptions {
            force: false,
            protected_session: self.current_session_target(),
        };
        let current_cwd = self.info.runtime.cwd.clone();
        let status = format!("deleting {}", plural(request.session_count(), "session"));
        // The daemon holds every session it has loaded, such as the ones
        // this TUI left: only it can delete those.
        let held = self.info.session.daemon.deleter();
        self.pending_sessions_task = Some(PendingSessionsTask {
            handle: tokio::task::spawn_blocking(move || {
                let report = request.run(options, held);
                let relisted = match origin {
                    DeleteOrigin::Hub => Relisted::Hub(load_hub_groups(&current_cwd)),
                    DeleteOrigin::Resume => {
                        Relisted::Resume(Session::list(&current_cwd, SessionListing::PersonFacing))
                    }
                };
                TaskOutput::Delete { report, relisted }
            }),
            kind: TaskKind::Delete,
        });
        self.set_status(status);
    }

    /// Apply a finished task from the running-turn loop, which has no
    /// `select!` arm for it. Returns true when it changed the UI.
    pub(super) fn poll_sessions_task(&mut self) -> anyhow::Result<bool> {
        use futures_util::FutureExt;
        if !self
            .pending_sessions_task
            .as_ref()
            .is_some_and(|task| task.handle.is_finished())
        {
            return Ok(false);
        }
        let Some(output) = next_sessions_task(&mut self.pending_sessions_task).now_or_never()
        else {
            return Ok(false);
        };
        self.finish_sessions_task(output)?;
        Ok(true)
    }

    /// Apply a finished sessions task.
    pub(super) fn finish_sessions_task(
        &mut self,
        output: Result<TaskOutput, JoinError>,
    ) -> anyhow::Result<()> {
        match output {
            Ok(TaskOutput::Load(groups)) => self.fill_sessions_hub(groups),
            Ok(TaskOutput::Delete { report, relisted }) => {
                let notice = self.report_sessions_delete(report);
                self.apply_relisted(relisted)?;
                self.set_status(notice);
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not update sessions: background task failed: {error}"
                )));
                self.set_status("sessions failed");
            }
        }
        Ok(())
    }

    /// Refresh the picker that started a delete, if it is still open.
    fn apply_relisted(&mut self, relisted: Relisted) -> anyhow::Result<()> {
        match (relisted, self.input_ui.composer()) {
            (Relisted::Hub(groups), ComposerMode::Picker(picker))
                if picker.is_manage_sessions() =>
            {
                match groups {
                    Ok(groups) => self.refresh_sessions_hub(groups),
                    Err(error) => self.insert_entry(&Entry::Error(format!(
                        "could not refresh sessions: {error}"
                    ))),
                }
            }
            (Relisted::Resume(sessions), ComposerMode::Picker(picker))
                if picker.is_resume_session() =>
            {
                let cursor = picker.cursor();
                self.show_resume_picker(sessions)?;
                if let ComposerMode::Picker(open) = self.input_ui.composer_mut() {
                    open.restore_cursor(&cursor);
                }
            }
            // The picker closed while the delete ran; the notice is enough.
            _ => {}
        }
        Ok(())
    }

    /// Put per-session failures in the transcript and return the status notice.
    fn report_sessions_delete(&mut self, report: DeleteReport) -> String {
        match report {
            DeleteReport::One { target, result } => {
                let short = session_picker::short_session_id(&target.id);
                match result {
                    Ok(outcome) => {
                        let mut notice = format!("deleted session {short}");
                        for (count, noun) in [
                            (outcome.child_sessions, "child session"),
                            (outcome.runs, "related run"),
                        ] {
                            if count > 0 {
                                notice.push_str(&format!(" and {}", plural(count, noun)));
                            }
                        }
                        notice
                    }
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not delete session: {error:#}"
                        )));
                        "delete failed".into()
                    }
                }
            }
            DeleteReport::Directory { cwd, result } => {
                let outcome = match result {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not delete sessions: {error}"
                        )));
                        return "delete failed".into();
                    }
                };
                for failure in &outcome.failures {
                    self.insert_entry(&Entry::Error(format!(
                        "could not delete session {}: {}",
                        session_picker::short_session_id(&failure.id),
                        failure.error
                    )));
                }
                let mut notice = format!(
                    "deleted {} in {}",
                    plural(outcome.deleted, "session"),
                    compact_cwd(&cwd)
                );
                if outcome.kept_current {
                    notice.push_str(", kept the current session");
                }
                if !outcome.failures.is_empty() {
                    notice.push_str(&format!(", {} failed", outcome.failures.len()));
                }
                notice
            }
            DeleteReport::CleanupMissing(result) => {
                let outcome = match result {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not clean up sessions: {error}"
                        )));
                        return "cleanup failed".into();
                    }
                };
                for failure in &outcome.failures {
                    self.insert_entry(&Entry::Error(format!(
                        "could not delete session {} ({}): {}",
                        session_picker::short_session_id(&failure.id),
                        compact_cwd(&failure.cwd),
                        failure.error
                    )));
                }
                let mut notice = format!("cleaned up {}", plural(outcome.deleted, "session"));
                if !outcome.failures.is_empty() {
                    notice.push_str(&format!(", {} failed", outcome.failures.len()));
                }
                if outcome.restored_workspaces > 0 {
                    notice.push_str(&format!(
                        ", {} skipped after restore",
                        outcome.restored_workspaces
                    ));
                }
                notice
            }
        }
    }
}

#[cfg(test)]
#[path = "sessions_hub_tasks_tests.rs"]
mod tests;
