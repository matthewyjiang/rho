//! `/login cursor` for the external Cursor Agent runtime.
//!
//! This path never touches Rho's credential store. `cursor-agent login` owns
//! the browser OAuth and stores credentials in `~/.cursor`.

use ratatui::DefaultTerminal;

use crate::cursor_runtime::{
    auth::{self, CursorAuthError, CursorAuthStatus},
    executable, models,
};

use super::{
    external_login::{
        AfterSuccessFuture, CompleteAnnouncement, ExternalLoginSpec, LoginAuthCopy, LoginConfirm,
    },
    App, Entry,
};

impl App {
    pub(super) async fn execute_cursor_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.start_external_login(terminal, cursor_login_spec())
            .await
    }

    pub(super) fn report_cursor_logout_unsupported(&mut self) {
        self.insert_entry(&Entry::Error(
            "could not log out of cursor: not available from rho, run cursor-agent logout".into(),
        ));
        self.set_status("logout failed");
    }
}

fn cursor_login_spec() -> ExternalLoginSpec<CursorAuthStatus, CursorAuthError> {
    ExternalLoginSpec {
        command_label: "cursor-agent login",
        resolve: executable::resolve,
        login_args: auth::login_args(),
        query: || Box::pin(auth::query()),
        is_signed_in: |status| status.is_authenticated,
        copy: LoginAuthCopy {
            status_line_prefix: "cursor login",
            signed_in_notice,
            incomplete_signed_out: |_| "could not complete cursor login: not signed in".into(),
            incomplete_query_error: |error| format!("could not complete cursor login: {error}"),
            failed: |error| format!("could not complete cursor login: {error:#}"),
            child_failed_but_signed_in: |_, status| signed_in_notice(status),
        },
        confirm: LoginConfirm::Direct,
        handoff_notice: "handing the terminal to cursor-agent login",
        handoff_status: "cursor-agent login".into(),
        complete_announcement: CompleteAnnouncement::NoticeAndStatus,
        after_success: Some(refresh_cursor_models_after_login),
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

fn refresh_cursor_models_after_login(_status: &CursorAuthStatus) -> AfterSuccessFuture {
    Box::pin(async {
        let _ = models::refresh_if_stale().await;
    })
}
