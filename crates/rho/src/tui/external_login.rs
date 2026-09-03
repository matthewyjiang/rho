//! Shared take-terminal-session login for external CLIs.
//!
//! Policy (program, args, copy, confirm-vs-direct, post-success) stays on
//! [`ExternalLoginSpec`]. This module owns suspend/spawn/restore and the
//! child-result × auth-probe state machine.

use std::future::Future;
use std::pin::Pin;

use anyhow::Context;
use ratatui::DefaultTerminal;

use crate::cli_runtime::CliExecutable;

use super::{external_editor, App, Entry};

pub(super) type QueryFuture<S, E> = Pin<Box<dyn Future<Output = Result<S, E>> + Send>>;
pub(super) type AfterSuccessFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Ask before the first handoff, or hand the terminal over immediately.
#[derive(Clone, Copy)]
pub(super) enum LoginConfirm<S, E> {
    /// Query first; confirm before handing off (Claude Code).
    Prompt {
        prompt_unsigned: fn(&mut App),
        prompt_signed_in: fn(&mut App, S),
        failed_status: &'static str,
        is_binary_missing: fn(&E) -> bool,
    },
    /// Hand off immediately (Cursor).
    Direct,
}

/// How a completed login is recorded in the transcript.
#[derive(Clone, Copy)]
pub(super) enum CompleteAnnouncement {
    /// Status bar only (Claude already inserted a handoff notice).
    StatusOnly,
    /// Transcript notice plus status bar (Cursor).
    NoticeAndStatus,
}

/// Copy and status-line prefix for one external login.
pub(super) struct LoginAuthCopy<S, E> {
    pub status_line_prefix: &'static str,
    pub signed_in_notice: fn(&S) -> String,
    pub incomplete_signed_out: fn(&S) -> String,
    pub incomplete_query_error: fn(&E) -> String,
    pub failed: fn(&anyhow::Error) -> String,
    pub child_failed_but_signed_in: fn(&anyhow::Error, &S) -> String,
}

/// Feature policy for one external CLI login.
pub(super) struct ExternalLoginSpec<S, E> {
    pub command_label: &'static str,
    pub resolve: fn() -> Result<CliExecutable, E>,
    pub login_args: &'static [&'static str],
    pub query: fn() -> QueryFuture<S, E>,
    pub is_signed_in: fn(&S) -> bool,
    pub copy: LoginAuthCopy<S, E>,
    pub confirm: LoginConfirm<S, E>,
    pub handoff_notice: &'static str,
    pub handoff_status: String,
    pub complete_announcement: CompleteAnnouncement,
    pub after_success: Option<fn(&S) -> AfterSuccessFuture>,
}

/// Post-login status outcome recorded in the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LoginAuthOutcome {
    Complete { notice: String },
    Incomplete { message: String },
    Failed { message: String },
}

/// Result of post-suspend login handling.
#[derive(Debug)]
enum LoginAfterSuspend<S> {
    ResumeFailed {
        error: anyhow::Error,
    },
    AuthResolved {
        outcome: LoginAuthOutcome,
        status: Option<S>,
    },
}

impl App {
    pub(super) async fn start_external_login<S, E>(
        &mut self,
        terminal: &mut DefaultTerminal,
        spec: ExternalLoginSpec<S, E>,
    ) -> anyhow::Result<()>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        match spec.confirm {
            LoginConfirm::Direct => self.run_external_login(terminal, spec).await,
            LoginConfirm::Prompt {
                prompt_unsigned,
                prompt_signed_in,
                failed_status,
                is_binary_missing,
            } => match (spec.query)().await {
                Ok(status) if (spec.is_signed_in)(&status) => {
                    prompt_signed_in(self, status);
                    Ok(())
                }
                Ok(_) => {
                    prompt_unsigned(self);
                    Ok(())
                }
                Err(error) if is_binary_missing(&error) => {
                    prompt_unsigned(self);
                    Ok(())
                }
                Err(error) => {
                    self.insert_entry(&Entry::Error(error.to_string()));
                    self.set_status(failed_status);
                    Ok(())
                }
            },
        }
    }

    pub(super) async fn run_external_login<S, E>(
        &mut self,
        terminal: &mut DefaultTerminal,
        spec: ExternalLoginSpec<S, E>,
    ) -> anyhow::Result<()>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.insert_entry(&Entry::Notice(spec.handoff_notice.into()));
        terminal.draw(|frame| self.draw(frame))?;

        let mut terminal_session = self
            .terminal_session
            .take()
            .context("terminal session is unavailable")?;
        let resolve = spec.resolve;
        let login_args = spec.login_args;
        let command_label = spec.command_label;
        let handoff_status = spec.handoff_status.clone();
        let suspended_run = terminal_session
            .run_suspended(terminal, &handoff_status, || async move {
                let executable = resolve().map_err(anyhow::Error::new)?;
                let mut command = executable
                    .try_command(login_args.iter().copied())
                    .map_err(anyhow::Error::new)?;
                command
                    .stdin(std::process::Stdio::inherit())
                    .stdout(std::process::Stdio::inherit())
                    .stderr(std::process::Stdio::inherit());
                #[cfg(unix)]
                let _signal_guard =
                    external_editor::unix_suspended_child_signals::SuspendedChildSignalGuard::install(
                        &mut command,
                    )
                    .with_context(|| {
                        format!("could not prepare {command_label} signal handling")
                    })?;
                let status = command.status().await.map_err(|source| {
                    anyhow::Error::from(source)
                        .context(format!("could not start {command_label}"))
                })?;
                if !status.success() {
                    return Err(anyhow::anyhow!("{command_label} exited with {status}"));
                }
                Ok(())
            })
            .await;
        self.terminal_session = Some(terminal_session);

        match resolve_login_after_suspend(
            suspended_run.resume_result,
            suspended_run.operation_result,
            spec.query,
            spec.is_signed_in,
            &spec.copy,
            spec.command_label,
        )
        .await
        {
            LoginAfterSuspend::ResumeFailed { error } => return Err(error),
            LoginAfterSuspend::AuthResolved { outcome, status } => {
                self.record_login_auth_outcome(
                    &outcome,
                    spec.copy.status_line_prefix,
                    spec.complete_announcement,
                );
                if let (LoginAuthOutcome::Complete { .. }, Some(status), Some(after_success)) =
                    (&outcome, status.as_ref(), spec.after_success)
                {
                    after_success(status).await;
                }
            }
        }

        self.ctrl_c_streak = 0;
        self.input_ui.clear_paste_burst();
        Ok(())
    }

    fn record_login_auth_outcome(
        &mut self,
        outcome: &LoginAuthOutcome,
        status_line_prefix: &'static str,
        announcement: CompleteAnnouncement,
    ) {
        match outcome {
            LoginAuthOutcome::Complete { notice } => {
                if matches!(announcement, CompleteAnnouncement::NoticeAndStatus) {
                    self.insert_entry(&Entry::Notice(notice.clone()));
                }
                self.set_status(notice);
            }
            LoginAuthOutcome::Incomplete { message } | LoginAuthOutcome::Failed { message } => {
                self.insert_entry(&Entry::Error(message.clone()));
                let suffix = match outcome {
                    LoginAuthOutcome::Incomplete { .. } => "incomplete",
                    LoginAuthOutcome::Failed { .. } => "failed",
                    LoginAuthOutcome::Complete { .. } => unreachable!(),
                };
                self.set_status(format!("{status_line_prefix} {suffix}"));
            }
        }
    }
}

/// Check terminal resume before any auth status work or child-result UI.
///
/// `query` is injected so unit tests can prove resume failures never call it,
/// while successful resume always re-queries regardless of child exit.
async fn resolve_login_after_suspend<S, E, F, Fut>(
    resume_result: Result<(), anyhow::Error>,
    operation_result: Result<(), anyhow::Error>,
    query: F,
    is_signed_in: fn(&S) -> bool,
    copy: &LoginAuthCopy<S, E>,
    command_label: &'static str,
) -> LoginAfterSuspend<S>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<S, E>>,
{
    if let Err(resume_error) = resume_result {
        let error = match operation_result {
            Ok(()) => resume_error,
            Err(operation_error) => {
                resume_error.context(format!("{command_label} also failed: {operation_error:#}"))
            }
        };
        return LoginAfterSuspend::ResumeFailed { error };
    }

    let (outcome, status) =
        resolve_login_auth_outcome(operation_result, query, is_signed_in, copy).await;
    LoginAfterSuspend::AuthResolved { outcome, status }
}

/// Map login child result plus a fresh auth status probe into UI state.
///
/// `query` is injected so unit tests can cover the state machine without
/// spawning an external CLI or reading personal auth.
async fn resolve_login_auth_outcome<S, E, F, Fut>(
    operation_result: Result<(), anyhow::Error>,
    query: F,
    is_signed_in: fn(&S) -> bool,
    copy: &LoginAuthCopy<S, E>,
) -> (LoginAuthOutcome, Option<S>)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<S, E>>,
{
    match operation_result {
        Ok(()) => match query().await {
            Ok(status) if is_signed_in(&status) => {
                let notice = (copy.signed_in_notice)(&status);
                (LoginAuthOutcome::Complete { notice }, Some(status))
            }
            Ok(status) => (
                LoginAuthOutcome::Incomplete {
                    message: (copy.incomplete_signed_out)(&status),
                },
                Some(status),
            ),
            Err(error) => (
                LoginAuthOutcome::Incomplete {
                    message: (copy.incomplete_query_error)(&error),
                },
                None,
            ),
        },
        Err(error) => match query().await {
            Ok(status) if is_signed_in(&status) => {
                let notice = (copy.child_failed_but_signed_in)(&error, &status);
                (LoginAuthOutcome::Complete { notice }, Some(status))
            }
            Ok(status) => (
                LoginAuthOutcome::Failed {
                    message: (copy.failed)(&error),
                },
                Some(status),
            ),
            Err(_) => (
                LoginAuthOutcome::Failed {
                    message: (copy.failed)(&error),
                },
                None,
            ),
        },
    }
}

#[cfg(test)]
#[path = "external_login_tests.rs"]
mod tests;
