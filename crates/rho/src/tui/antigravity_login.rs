//! `/login antigravity` for the external Antigravity runtime.
//!
//! Hands the terminal to `agy acp --login`, agy's own Google sign-in, which
//! accepts a pasted code so it also works over SSH. Rho's credential store
//! is never touched; agy keeps the token, shared with the agy CLI.

use crate::tui::DefaultTerminal;

use crate::antigravity_runtime::{
    auth::{self, AntigravityProbeError, AntigravityStatus},
    executable,
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
        // Same gate as `rho login antigravity`: an agy without `agy acp`
        // must not get the terminal.
        if let Err(error) = auth::login_preflight().await {
            // The error already names antigravity (`antigravity: ...`).
            self.insert_entry(&Entry::Error(format!("could not log in to {error}")));
            self.set_status("antigravity login failed");
            return Ok(());
        }
        self.start_external_login(terminal, antigravity_login_spec())
            .await
    }

    pub(super) fn report_antigravity_logout_unsupported(&mut self) {
        self.insert_entry(&Entry::Error(
            "could not log out of antigravity: not available from rho, sign out with /logout in agy"
                .into(),
        ));
        self.set_status("logout failed");
    }
}

fn antigravity_login_spec() -> ExternalLoginSpec<AntigravityStatus, AntigravityProbeError> {
    ExternalLoginSpec {
        command_label: "agy acp --login",
        resolve: || executable::resolve().map_err(AntigravityProbeError::from),
        login_args: executable::LOGIN_ARGS,
        query: || Box::pin(auth::query()),
        is_signed_in: |status| status.signed_in,
        copy: LoginAuthCopy {
            status_line_prefix: "antigravity login",
            signed_in_notice,
            incomplete_signed_out: |_| "could not complete antigravity login: not signed in".into(),
            incomplete_query_error: |error| {
                format!("could not complete antigravity login: {error}")
            },
            failed: |error| format!("could not complete antigravity login: {error:#}"),
            child_failed_but_signed_in: |_, status| signed_in_notice(status),
        },
        confirm: LoginConfirm::Direct,
        handoff_notice: "handing the terminal to agy acp --login",
        handoff_status: "agy acp --login".into(),
        complete_announcement: CompleteAnnouncement::NoticeAndStatus,
        after_success: None,
    }
}

fn signed_in_notice(status: &AntigravityStatus) -> String {
    format!("signed in to antigravity (agy {})", status.version)
}
