//! `/login claude-code` and `/logout claude-code` for the external Claude Code runtime.
//!
//! This path never touches Rho's credential store. Claude Code owns the
//! sign-in, stores the token, and remains the source of truth after handoff.

use anyhow::Context;
use ratatui::DefaultTerminal;

use crate::claude_runtime::{
    auth::{self, ClaudeAuthError, ClaudeAuthStatus},
    executable,
};

use super::{
    external_editor, App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending,
};

/// Stable `/login` and `/logout` target for the Claude Code subscription runtime.
pub(super) const CLAUDE_CODE_TARGET: &str = "claude-code";

pub(super) const RELAY_LOGIN_VALUE: &str = "continue";
pub(super) const KEEP_LOGIN_VALUE: &str = "keep";
pub(super) const CONFIRM_LOGOUT_VALUE: &str = "confirm";
pub(super) const CANCEL_LOGOUT_VALUE: &str = "cancel";

/// Login methods that are not Rho provider credentials.
///
/// The picker renders whatever it is handed; which group Claude Code belongs to
/// is this feature's policy, not the picker's.
pub(super) const EXTERNAL_LOGIN_METHODS: &[ExternalLoginMethod] = &[ExternalLoginMethod {
    // Claude Code is an Anthropic-family runtime for delegation, not a separate
    // top-level provider group. Keep it beside the Anthropic API key method.
    group_id: "anthropic",
    value: CLAUDE_CODE_TARGET,
    label: "Claude Code (delegation only)",
    detail: "External Claude binary subscription, not Anthropic API billing. \
Credentials are managed by Claude Code, not Rho.",
}];

/// One login method backed by an external runtime rather than a Rho credential.
pub(super) struct ExternalLoginMethod {
    /// Login group this method is offered under.
    pub(super) group_id: &'static str,
    /// Picker value, which is also the `/login` argument.
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) detail: &'static str,
}

/// What a `/login` or `/logout` argument names.
///
/// Parsed once at each command or picker boundary so the provider flows never
/// re-sniff for the external runtime.
pub(super) enum SignInTarget {
    /// Claude Code, whose credential the `claude` binary owns.
    ClaudeCode,
    /// A Rho provider credential.
    Provider(String),
}

impl SignInTarget {
    pub(super) fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.eq_ignore_ascii_case(CLAUDE_CODE_TARGET) {
            Self::ClaudeCode
        } else {
            Self::Provider(value.to_string())
        }
    }
}

/// Post-login status outcome recorded in the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClaudeLoginAuthOutcome {
    Complete { notice: String },
    Incomplete { message: String },
    Failed { message: String },
}

impl ClaudeLoginAuthOutcome {
    fn status_line(&self) -> &'static str {
        match self {
            Self::Complete { .. } => "claude code login complete",
            Self::Incomplete { .. } => "claude code login incomplete",
            Self::Failed { .. } => "claude code login failed",
        }
    }
}

impl App {
    pub(super) async fn execute_claude_code_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match auth::query().await {
            Ok(status) if status.logged_in => {
                self.prompt_claude_code_relogin(status);
                Ok(())
            }
            Ok(_) | Err(ClaudeAuthError::BinaryMissing) => {
                // Missing binary still goes through the handoff path so the
                // user sees the ownership notice and a clear spawn failure.
                self.run_claude_code_login(terminal).await
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(error.to_string()));
                self.set_status("claude code login failed");
                Ok(())
            }
        }
    }

    pub(super) async fn submit_claude_code_relogin_choice(
        &mut self,
        choice: InlineChoice,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match choice.selected_value() {
            RELAY_LOGIN_VALUE => self.run_claude_code_login(terminal).await,
            _ => {
                self.set_status("claude code login unchanged");
                Ok(())
            }
        }
    }

    pub(super) fn prompt_claude_code_logout(&mut self) {
        let choice = InlineChoice::new(
            "Sign out of Claude Code everywhere?",
            auth::logout_confirm_description(),
            vec![
                InlineChoiceOption::available(
                    CANCEL_LOGOUT_VALUE,
                    '1',
                    "Cancel",
                    "Leave Claude Code signed in",
                )
                .with_alternate_shortcut('c'),
                InlineChoiceOption::available(
                    CONFIRM_LOGOUT_VALUE,
                    '2',
                    "Sign out everywhere",
                    "Run claude auth logout for this machine",
                )
                .with_alternate_shortcut('s'),
            ],
        )
        .expect("claude code logout choice has available options");
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ClaudeCodeLogout,
            }));
        self.set_status("confirm claude code logout");
    }

    pub(super) async fn submit_claude_code_logout_choice(
        &mut self,
        choice: InlineChoice,
    ) -> anyhow::Result<()> {
        match choice.selected_value() {
            CONFIRM_LOGOUT_VALUE => self.run_claude_code_logout().await,
            _ => {
                self.set_status("claude code logout cancelled");
                Ok(())
            }
        }
    }

    pub(super) async fn execute_claude_code_logout(&mut self) -> anyhow::Result<()> {
        self.prompt_claude_code_logout();
        Ok(())
    }

    async fn run_claude_code_logout(&mut self) -> anyhow::Result<()> {
        // Always run logout, then treat a fresh status query as the truth.
        // Child failure is extra detail only.
        let logout_result = auth::logout().await;
        if let Err(ClaudeAuthError::BinaryMissing) = &logout_result {
            self.insert_entry(&Entry::Error(ClaudeAuthError::BinaryMissing.to_string()));
            self.set_status("claude code logout failed");
            return Ok(());
        }

        match auth::query().await {
            Ok(status) if !status.logged_in => {
                let mut notice =
                    "claude code: signed out everywhere the claude binary is used".to_string();
                if let Err(error) = &logout_result {
                    notice.push_str(&format!(
                        "\nclaude auth logout also reported: {}",
                        error.sanitized_detail()
                    ));
                }
                self.insert_entry(&Entry::Notice(notice));
                self.set_status("claude code logout complete");
            }
            Ok(status) => {
                let mut message = format!(
                    "claude auth logout finished, but status still reports signed in ({})",
                    status.describe()
                );
                if let Err(error) = &logout_result {
                    message.push_str(&format!("\nchild detail: {}", error.sanitized_detail()));
                }
                self.insert_entry(&Entry::Error(message));
                self.set_status("claude code logout incomplete");
            }
            Err(error) => {
                let mut message =
                    format!("claude auth logout finished, but status could not be read: {error}");
                if let Err(child) = &logout_result {
                    message.push_str(&format!("\nchild detail: {}", child.sanitized_detail()));
                } else if let Some(excerpt) = error.stderr_excerpt() {
                    message.push_str(&format!("\nstderr: {excerpt}"));
                } else {
                    message.push_str(&format!("\ndetail: {}", error.sanitized_detail()));
                }
                self.insert_entry(&Entry::Error(message));
                self.set_status("claude code logout incomplete");
            }
        }
        Ok(())
    }

    fn prompt_claude_code_relogin(&mut self, status: ClaudeAuthStatus) {
        let choice = InlineChoice::new(
            "Claude Code is already signed in",
            format!(
                "{}\nContinue only if you want to re-run `claude auth login --claudeai`.",
                status.describe()
            ),
            vec![
                InlineChoiceOption::available(
                    KEEP_LOGIN_VALUE,
                    '1',
                    "Keep current sign-in",
                    "Leave Claude Code as it is",
                )
                .with_alternate_shortcut('k'),
                InlineChoiceOption::available(
                    RELAY_LOGIN_VALUE,
                    '2',
                    "Sign in again",
                    "Hand the terminal to claude auth login",
                )
                .with_alternate_shortcut('s'),
            ],
        )
        .expect("claude code relogin choice has available options");
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ClaudeCodeRelogin,
            }));
        self.set_status("claude code already signed in");
    }

    async fn run_claude_code_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.insert_entry(&Entry::Notice(auth::login_handoff_notice().into()));
        // Draw the ownership notice before leaving the alternate screen so the
        // handoff message is part of session history after resume.
        terminal.draw(|frame| self.draw(frame))?;

        let mut terminal_session = self
            .terminal_session
            .take()
            .context("terminal session is unavailable")?;
        let suspended_run = terminal_session
            .run_suspended(terminal, "Signing in to Claude Code…", || async move {
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
                    .context("could not prepare claude login signal handling")?;
                let status = command.status().await.map_err(|source| {
                    if source.kind() == std::io::ErrorKind::NotFound {
                        anyhow::Error::new(ClaudeAuthError::BinaryMissing)
                    } else {
                        anyhow::Error::from(source).context("could not start claude auth login")
                    }
                })?;
                if !status.success() {
                    return Err(anyhow::anyhow!("claude auth login exited with {status}"));
                }
                Ok(())
            })
            .await;
        self.terminal_session = Some(terminal_session);

        // resume_result owns terminal lifecycle and must win first. Auth status
        // and child-exit presentation are only safe once the TUI is back.
        match resolve_claude_login_after_suspend(
            suspended_run.resume_result,
            suspended_run.operation_result,
            auth::query,
        )
        .await
        {
            ClaudeLoginAfterSuspend::ResumeFailed { error } => return Err(error),
            ClaudeLoginAfterSuspend::AuthResolved { outcome } => {
                self.record_claude_login_auth_outcome(&outcome);
            }
        }

        self.ctrl_c_streak = 0;
        self.input_ui.clear_paste_burst();
        Ok(())
    }

    fn record_claude_login_auth_outcome(&mut self, outcome: &ClaudeLoginAuthOutcome) {
        match outcome {
            ClaudeLoginAuthOutcome::Complete { notice } => {
                self.set_status(notice);
            }
            ClaudeLoginAuthOutcome::Incomplete { message }
            | ClaudeLoginAuthOutcome::Failed { message } => {
                self.insert_entry(&Entry::Error(message.clone()));
                self.set_status(outcome.status_line());
            }
        }
    }
}

/// Result of post-suspend Claude login handling.
#[derive(Debug)]
enum ClaudeLoginAfterSuspend {
    ResumeFailed { error: anyhow::Error },
    AuthResolved { outcome: ClaudeLoginAuthOutcome },
}

/// Check terminal resume before any auth status work or child-result UI.
///
/// `query` is injected so unit tests can prove resume failures never call it,
/// while successful resume always re-queries regardless of child exit.
async fn resolve_claude_login_after_suspend<F, Fut>(
    resume_result: Result<(), anyhow::Error>,
    operation_result: Result<(), anyhow::Error>,
    query: F,
) -> ClaudeLoginAfterSuspend
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<ClaudeAuthStatus, ClaudeAuthError>>,
{
    if let Err(resume_error) = resume_result {
        let error = match operation_result {
            Ok(()) => resume_error,
            Err(operation_error) => resume_error.context(format!(
                "claude auth login also failed: {operation_error:#}"
            )),
        };
        return ClaudeLoginAfterSuspend::ResumeFailed { error };
    }

    ClaudeLoginAfterSuspend::AuthResolved {
        outcome: resolve_claude_login_auth_outcome(operation_result, query).await,
    }
}

/// Map login child result plus a fresh auth status probe into UI state.
///
/// `query` is injected so unit tests can cover the state machine without
/// spawning `claude` or reading personal auth.
async fn resolve_claude_login_auth_outcome<F, Fut>(
    operation_result: Result<(), anyhow::Error>,
    query: F,
) -> ClaudeLoginAuthOutcome
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<ClaudeAuthStatus, ClaudeAuthError>>,
{
    match operation_result {
        Ok(()) => match query().await {
            Ok(status) if status.logged_in => ClaudeLoginAuthOutcome::Complete {
                notice: status.describe_login_success(),
            },
            Ok(status) => ClaudeLoginAuthOutcome::Incomplete {
                message: format!(
                    "claude auth login finished, but status still reports signed out ({})",
                    status.describe()
                ),
            },
            Err(error) => ClaudeLoginAuthOutcome::Incomplete {
                message: format!(
                    "claude auth login finished, but status could not be read: {error}"
                ),
            },
        },
        Err(error) => {
            // Prefer a post-status check when the child failed; the user may
            // already be signed in from a previous attempt.
            match query().await {
                Ok(status) if status.logged_in => ClaudeLoginAuthOutcome::Complete {
                    notice: format!(
                        "claude auth login reported an error ({error:#}), but status shows signed in.\n{}",
                        status.describe_login_success()
                    ),
                },
                _ => ClaudeLoginAuthOutcome::Failed {
                    message: format!("claude code login failed: {error:#}"),
                },
            }
        }
    }
}

#[cfg(test)]
#[path = "claude_login_tests.rs"]
mod tests;
