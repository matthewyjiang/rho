//! `/login cursor` for the external Cursor Agent runtime.
//!
//! This path never touches Rho's credential store. `cursor-agent login` owns
//! the browser OAuth and stores credentials in `~/.cursor`.

use anyhow::Context;
use ratatui::DefaultTerminal;

use crate::cursor_runtime::{
    auth::{self, CursorAuthError, CursorAuthStatus},
    executable,
};

use super::{external_editor, App, Entry};

/// Stable `/login` target for the Cursor Agent CLI runtime.
pub(super) const CURSOR_TARGET: &str = "cursor";
const CURSOR_AGENT_ALIAS: &str = "cursor-agent";

pub(super) fn is_cursor_login_target(value: &str) -> bool {
    value.eq_ignore_ascii_case(CURSOR_TARGET) || value.eq_ignore_ascii_case(CURSOR_AGENT_ALIAS)
}

/// Post-login status outcome recorded in the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CursorLoginAuthOutcome {
    Complete { notice: String },
    Incomplete { message: String },
    Failed { message: String },
}

impl CursorLoginAuthOutcome {
    fn status_line(&self) -> &'static str {
        match self {
            Self::Complete { .. } => "cursor login complete",
            Self::Incomplete { .. } => "cursor login incomplete",
            Self::Failed { .. } => "cursor login failed",
        }
    }
}

impl App {
    pub(super) async fn execute_cursor_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if let Err(CursorAuthError::BinaryMissing) = executable::resolve() {
            self.insert_entry(&Entry::Error(
                "could not start cursor login: cursor-agent not found on PATH".into(),
            ));
            self.set_status("cursor login failed");
            return Ok(());
        }
        self.run_cursor_login(terminal).await
    }

    pub(super) fn report_cursor_logout_unsupported(&mut self) {
        self.insert_entry(&Entry::Error(
            "could not log out of cursor: not available from rho, run cursor-agent logout".into(),
        ));
        self.set_status("logout failed");
    }

    async fn run_cursor_login(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        self.insert_entry(&Entry::Notice(
            "handing the terminal to cursor-agent login".into(),
        ));
        terminal.draw(|frame| self.draw(frame))?;

        let mut terminal_session = self
            .terminal_session
            .take()
            .context("terminal session is unavailable")?;
        let suspended_run = terminal_session
            .run_suspended(terminal, "cursor-agent login", || async move {
                let executable = executable::resolve().map_err(anyhow::Error::new)?;
                let mut command = executable
                    .try_command(auth::login_args().iter().copied())
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
                    .context("could not prepare cursor login signal handling")?;
                let status = command.status().await.map_err(|source| {
                    if source.kind() == std::io::ErrorKind::NotFound {
                        anyhow::anyhow!(
                            "could not start cursor login: cursor-agent not found on PATH"
                        )
                    } else {
                        anyhow::Error::from(source).context("could not start cursor-agent login")
                    }
                })?;
                if !status.success() {
                    return Err(anyhow::anyhow!("cursor-agent login exited with {status}"));
                }
                Ok(())
            })
            .await;
        self.terminal_session = Some(terminal_session);

        match resolve_cursor_login_after_suspend(
            suspended_run.resume_result,
            suspended_run.operation_result,
            auth::query,
        )
        .await
        {
            CursorLoginAfterSuspend::ResumeFailed { error } => return Err(error),
            CursorLoginAfterSuspend::AuthResolved { outcome } => {
                let complete = matches!(outcome, CursorLoginAuthOutcome::Complete { .. });
                self.record_cursor_login_auth_outcome(&outcome);
                if complete {
                    let _ = crate::cursor_runtime::models::refresh().await;
                }
            }
        }

        self.ctrl_c_streak = 0;
        self.input_ui.clear_paste_burst();
        Ok(())
    }

    fn record_cursor_login_auth_outcome(&mut self, outcome: &CursorLoginAuthOutcome) {
        match outcome {
            CursorLoginAuthOutcome::Complete { notice } => {
                self.insert_entry(&Entry::Notice(notice.clone()));
                self.set_status(notice);
            }
            CursorLoginAuthOutcome::Incomplete { message }
            | CursorLoginAuthOutcome::Failed { message } => {
                self.insert_entry(&Entry::Error(message.clone()));
                self.set_status(outcome.status_line());
            }
        }
    }
}

#[derive(Debug)]
enum CursorLoginAfterSuspend {
    ResumeFailed { error: anyhow::Error },
    AuthResolved { outcome: CursorLoginAuthOutcome },
}

async fn resolve_cursor_login_after_suspend<F, Fut>(
    resume_result: Result<(), anyhow::Error>,
    operation_result: Result<(), anyhow::Error>,
    query: F,
) -> CursorLoginAfterSuspend
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<CursorAuthStatus, CursorAuthError>>,
{
    if let Err(resume_error) = resume_result {
        let error = match operation_result {
            Ok(()) => resume_error,
            Err(operation_error) => resume_error.context(format!(
                "cursor-agent login also failed: {operation_error:#}"
            )),
        };
        return CursorLoginAfterSuspend::ResumeFailed { error };
    }

    CursorLoginAfterSuspend::AuthResolved {
        outcome: resolve_cursor_login_auth_outcome(operation_result, query).await,
    }
}

/// Map login child result plus a fresh auth status probe into UI state.
///
/// `query` is injected so unit tests can cover the two status JSON shapes
/// without spawning `cursor-agent`.
async fn resolve_cursor_login_auth_outcome<F, Fut>(
    operation_result: Result<(), anyhow::Error>,
    query: F,
) -> CursorLoginAuthOutcome
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<CursorAuthStatus, CursorAuthError>>,
{
    match operation_result {
        Ok(()) => match query().await {
            Ok(status) if status.is_authenticated => CursorLoginAuthOutcome::Complete {
                notice: signed_in_notice(&status),
            },
            Ok(_) => CursorLoginAuthOutcome::Incomplete {
                message: "could not complete cursor login: not signed in".into(),
            },
            Err(error) => CursorLoginAuthOutcome::Incomplete {
                message: format!("could not complete cursor login: {error}"),
            },
        },
        Err(error) => match query().await {
            Ok(status) if status.is_authenticated => CursorLoginAuthOutcome::Complete {
                notice: signed_in_notice(&status),
            },
            _ => CursorLoginAuthOutcome::Failed {
                message: format!("could not complete cursor login: {error:#}"),
            },
        },
    }
}

fn signed_in_notice(status: &CursorAuthStatus) -> String {
    match status
        .user_info
        .as_ref()
        .and_then(|info| info.email.as_deref())
        .filter(|email| !email.is_empty())
    {
        Some(email) => format!("signed in to cursor as {email}"),
        None => "signed in to cursor".into(),
    }
}

#[cfg(test)]
#[path = "cursor_login_tests.rs"]
mod tests;
