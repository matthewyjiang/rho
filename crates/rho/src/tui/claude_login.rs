//! `/login claude-code` and `/logout claude-code` for the external Claude Code runtime.
//!
//! This path never touches Rho's credential store. Claude Code owns the
//! sign-in, stores the token, and remains the source of truth after handoff.

use ratatui::DefaultTerminal;

use crate::claude_runtime::{
    auth::{self, ClaudeAuthError, ClaudeAuthStatus},
    executable,
};

use super::{
    external_login::{CompleteAnnouncement, ExternalLoginSpec, LoginAuthCopy, LoginConfirm},
    App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending,
};

/// Stable `/login` and `/logout` target for the Claude Code subscription runtime.
pub(super) const CLAUDE_CODE_TARGET: &str = "claude-code";

pub(super) const RELAY_LOGIN_VALUE: &str = "continue";
pub(super) const KEEP_LOGIN_VALUE: &str = "keep";
pub(super) const CANCEL_LOGIN_VALUE: &str = "cancel";
pub(super) const CONFIRM_LOGOUT_VALUE: &str = "confirm";
pub(super) const CANCEL_LOGOUT_VALUE: &str = "cancel";

impl App {
    pub(super) async fn execute_claude_code_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.start_external_login(terminal, claude_login_spec())
            .await
    }

    pub(super) async fn submit_claude_code_login_choice(
        &mut self,
        choice: InlineChoice,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match choice.selected_value() {
            RELAY_LOGIN_VALUE => self.run_external_login(terminal, claude_login_spec()).await,
            _ => {
                self.set_status("claude code login cancelled");
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
            RELAY_LOGIN_VALUE => self.run_external_login(terminal, claude_login_spec()).await,
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
                parent_picker: None,
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

    fn prompt_claude_code_login(&mut self) {
        let choice = InlineChoice::new(
            "Hand the terminal to Claude Code?",
            "Rho will suspend and run `claude auth login --claudeai`. \
Cancel if you did not mean to sign in.",
            vec![
                InlineChoiceOption::available(CANCEL_LOGIN_VALUE, '1', "Cancel", "Stay in Rho")
                    .with_alternate_shortcut('n'),
                InlineChoiceOption::available(
                    RELAY_LOGIN_VALUE,
                    '2',
                    "Continue",
                    "Hand the terminal to claude auth login",
                )
                .with_alternate_shortcut('s'),
            ],
        )
        .expect("claude code login choice has available options");
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::ClaudeCodeLogin,
                parent_picker: None,
            }));
        self.set_status("confirm claude code login");
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
                parent_picker: None,
            }));
        self.set_status("claude code already signed in");
    }
}

fn claude_login_spec() -> ExternalLoginSpec<ClaudeAuthStatus, ClaudeAuthError> {
    ExternalLoginSpec {
        command_label: "claude auth login",
        resolve: executable::resolve,
        login_args: auth::login_args(),
        query: || Box::pin(auth::query()),
        is_signed_in: |status| status.logged_in,
        copy: LoginAuthCopy {
            status_line_prefix: "claude code login",
            signed_in_notice: ClaudeAuthStatus::describe_login_success,
            incomplete_signed_out: |status| {
                format!(
                    "claude auth login finished, but status still reports signed out ({})",
                    status.describe()
                )
            },
            incomplete_query_error: |error| {
                format!("claude auth login finished, but status could not be read: {error}")
            },
            failed: |error| format!("claude code login failed: {error:#}"),
            child_failed_but_signed_in: |error, status| {
                format!(
                    "claude auth login reported an error ({error:#}), but status shows signed in.\n{}",
                    status.describe_login_success()
                )
            },
        },
        confirm: LoginConfirm::Prompt {
            prompt_unsigned: App::prompt_claude_code_login,
            prompt_signed_in: App::prompt_claude_code_relogin,
            failed_status: "claude code login failed",
            is_binary_missing: |error| matches!(error, ClaudeAuthError::BinaryMissing),
        },
        handoff_notice: auth::login_handoff_notice(),
        handoff_status: auth::login_handoff_status(),
        complete_announcement: CompleteAnnouncement::StatusOnly,
        after_success: None,
    }
}
