//! `/login antigravity` for the external Antigravity runtime.
//!
//! Hands the terminal to `rho login antigravity` (this same binary), which
//! offers to install a missing server, then drives the server's Google
//! sign-in and accepts a pasted redirect over SSH.
//! Rho's credential store is never touched; the server keeps its token under
//! the Gemini home.
//!
//! Login counts as complete only when runs can start (server installed and
//! signed in), so a declined or failed install never reads as signed in on
//! the strength of an earlier sign-in.

use crate::tui::DefaultTerminal;

use crate::{
    antigravity_runtime::{
        home::{AntigravityAuthStatus, AntigravityHome},
        setup::AntigravitySetup,
    },
    cli_runtime::CliExecutable,
};

use super::{
    external_login::{CompleteAnnouncement, ExternalLoginSpec, LoginAuthCopy, LoginConfirm},
    App, Entry,
};

impl App {
    pub(super) async fn execute_antigravity_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.start_external_login(terminal, antigravity_login_spec())
            .await
    }

    pub(super) fn report_antigravity_logout_unsupported(&mut self) {
        let token = AntigravityHome::from_env(&crate::paths::home_dir().unwrap_or_default());
        self.insert_entry(&Entry::Error(format!(
            "could not log out of antigravity: not available from rho, delete the token in {}",
            crate::paths::display(token.acp_dir())
        )));
        self.set_status("logout failed");
    }
}

#[derive(Debug, thiserror::Error)]
enum AntigravityLoginError {
    #[error("could not locate the running rho binary: {0}")]
    RhoExecutable(std::io::Error),
}

/// Run this binary; it checks for (and offers to install) the server.
fn resolve_login_command() -> Result<CliExecutable, AntigravityLoginError> {
    std::env::current_exe()
        .map(CliExecutable::from_path)
        .map_err(AntigravityLoginError::RhoExecutable)
}

fn antigravity_login_spec() -> ExternalLoginSpec<AntigravitySetup, AntigravityLoginError> {
    ExternalLoginSpec {
        command_label: "rho login antigravity",
        resolve: resolve_login_command,
        login_args: &["login", "antigravity"],
        query: || Box::pin(async { Ok(setup_from_env()) }),
        is_signed_in: AntigravitySetup::is_ready,
        copy: LoginAuthCopy {
            status_line_prefix: "antigravity login",
            signed_in_notice,
            incomplete_signed_out: |setup| {
                format!(
                    "could not complete antigravity login: {}",
                    setup.description()
                )
            },
            incomplete_query_error: |error| {
                format!("could not complete antigravity login: {error}")
            },
            // The child's own output is gone once the TUI redraws; keep the
            // likeliest cause (install declined or failed) in the transcript.
            failed: |error| match setup_from_env() {
                setup if setup.is_ready() => {
                    format!("could not complete antigravity login: {error:#}")
                }
                setup => format!(
                    "could not complete antigravity login: {} ({error:#})",
                    setup.description()
                ),
            },
            child_failed_but_signed_in: |_, status| signed_in_notice(status),
        },
        confirm: LoginConfirm::Direct,
        handoff_notice: "handing the terminal to rho login antigravity",
        handoff_status: "rho login antigravity".into(),
        complete_announcement: CompleteAnnouncement::NoticeAndStatus,
        after_success: None,
    }
}

fn setup_from_env() -> AntigravitySetup {
    AntigravitySetup::from_env(&crate::paths::home_dir().unwrap_or_default())
}

fn signed_in_notice(setup: &AntigravitySetup) -> String {
    match &setup.auth {
        AntigravityAuthStatus::Configured { method } => {
            format!("signed in to antigravity ({method})")
        }
        AntigravityAuthStatus::SignedOut { .. }
        | AntigravityAuthStatus::MissingToken { .. }
        | AntigravityAuthStatus::Unreadable { .. } => "signed in to antigravity".into(),
    }
}
