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
    CleanupOutcome, DeleteOptions, DeleteOutcome, SessionSummary, SessionTarget, Workspace,
    WorkspaceDeleteOutcome,
};

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

    fn run(self, options: DeleteOptions) -> DeleteReport {
        match self {
            Self::One(target) => DeleteReport::One {
                result: Session::delete_target(&target, options),
                target,
            },
            Self::Directory { cwd, targets } => DeleteReport::Directory {
                result: Session::delete_targets(&targets, options),
                cwd,
            },
            Self::CleanupMissing(targets) => {
                DeleteReport::CleanupMissing(Session::cleanup_missing_targets(&targets, options))
            }
        }
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
        result: anyhow::Result<DeleteOutcome>,
    },
    Directory {
        cwd: PathBuf,
        result: anyhow::Result<WorkspaceDeleteOutcome>,
    },
    CleanupMissing(anyhow::Result<CleanupOutcome>),
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

/// Lists and groups every saved session. Blocking: reconciles the session
/// index and stats every workspace directory.
fn load_hub_groups(current_cwd: &Path) -> anyhow::Result<Vec<HubGroup>> {
    let sessions = Session::list_all()?;
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
        self.pending_sessions_task = Some(PendingSessionsTask {
            handle: tokio::task::spawn_blocking(move || {
                let report = request.run(options);
                let relisted = match origin {
                    DeleteOrigin::Hub => Relisted::Hub(load_hub_groups(&current_cwd)),
                    DeleteOrigin::Resume => Relisted::Resume(Session::list(&current_cwd)),
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
                    Ok(outcome) if outcome.deleted_run_count > 0 => format!(
                        "deleted session {short} and {}",
                        plural(outcome.deleted_run_count, "related run")
                    ),
                    Ok(_) => format!("deleted session {short}"),
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not delete session: {error}"
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
                    plural(outcome.deleted.len(), "session"),
                    compact_cwd(&cwd)
                );
                if !outcome.kept_protected.is_empty() {
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
                let mut notice = format!("cleaned up {}", plural(outcome.deleted.len(), "session"));
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
