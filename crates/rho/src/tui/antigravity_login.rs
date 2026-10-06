//! `/login antigravity` for the external Antigravity runtime.
//!
//! Hands the terminal to `rho login antigravity` (this same binary), which
//! drives the server's Google sign-in and accepts a pasted redirect over SSH.
//! Rho's credential store is never touched; the server keeps its token under
//! the Gemini home.

use crate::tui::DefaultTerminal;

use crate::{
    antigravity_runtime::{
        executable::{self, AntigravityExecutableError},
        home::{AntigravityAuthStatus, AntigravityHome},
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
    #[error(transparent)]
    ServerMissing(#[from] AntigravityExecutableError),
    #[error("could not locate the running rho binary: {0}")]
    RhoExecutable(std::io::Error),
}

/// Check the server exists before the handoff, then run this binary.
fn resolve_login_command() -> Result<CliExecutable, AntigravityLoginError> {
    executable::resolve()?;
    std::env::current_exe()
        .map(CliExecutable::from_path)
        .map_err(AntigravityLoginError::RhoExecutable)
}

fn antigravity_login_spec() -> ExternalLoginSpec<AntigravityAuthStatus, AntigravityLoginError> {
    ExternalLoginSpec {
        command_label: "rho login antigravity",
        resolve: resolve_login_command,
        login_args: &["login", "antigravity"],
        query: || {
            Box::pin(async {
                Ok(
                    AntigravityHome::from_env(&crate::paths::home_dir().unwrap_or_default())
                        .status(),
                )
            })
        },
        is_signed_in: AntigravityAuthStatus::is_signed_in,
        copy: LoginAuthCopy {
            status_line_prefix: "antigravity login",
            signed_in_notice,
            incomplete_signed_out: |status| {
                let detail = status.require_signed_in().err().unwrap_or_default();
                format!("could not complete antigravity login: {detail}")
            },
            incomplete_query_error: |error| {
                format!("could not complete antigravity login: {error}")
            },
            failed: |error| format!("could not complete antigravity login: {error:#}"),
            child_failed_but_signed_in: |_, status| signed_in_notice(status),
        },
        confirm: LoginConfirm::Direct,
        handoff_notice: "handing the terminal to rho login antigravity",
        handoff_status: "rho login antigravity".into(),
        complete_announcement: CompleteAnnouncement::NoticeAndStatus,
        after_success: None,
    }
}

fn signed_in_notice(status: &AntigravityAuthStatus) -> String {
    match status {
        AntigravityAuthStatus::Configured { method } => {
            format!("signed in to antigravity ({method})")
        }
        AntigravityAuthStatus::SignedOut { .. }
        | AntigravityAuthStatus::MissingToken { .. }
        | AntigravityAuthStatus::Unreadable { .. } => "signed in to antigravity".into(),
    }
}
