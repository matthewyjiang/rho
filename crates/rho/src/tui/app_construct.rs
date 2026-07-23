//! App construction helpers for the interactive TUI.

use std::sync::Arc;

use rho_providers::credentials::{available_auth_modes, CredentialStore};

use crate::credential_store::AppCredentialStore;

use super::{
    app_state::{HistoryUi, InputUi, PendingWorkUi, TurnUi},
    clipboard::SystemClipboard,
    feed_image::picker_from_environment,
    statusline::StatusLine,
    subagent_panel::SubagentPanel,
    App, StreamUi, TuiBootstrap, UsageUi,
};

#[cfg(debug_assertions)]
use super::smoke_injection;

impl App {
    pub(super) fn new(
        info: TuiBootstrap,
        herdr_graphics: crate::herdr::HerdrGraphicsCapability,
    ) -> Self {
        #[cfg(debug_assertions)]
        if smoke_injection::matrix_enabled() {
            return Self::new_with_credentials(
                info,
                Arc::new(rho_providers::credentials::MemoryCredentialStore::default()),
                herdr_graphics,
            );
        }
        Self::new_with_credentials(info, Arc::new(AppCredentialStore), herdr_graphics)
    }

    pub(super) fn new_with_credentials(
        info: TuiBootstrap,
        credential_store: Arc<dyn CredentialStore>,
        herdr_graphics: crate::herdr::HerdrGraphicsCapability,
    ) -> Self {
        let available_auths = available_auth_modes(credential_store.as_ref());
        let using_unavailable_provider = info.services.auth_unavailable.is_some();
        let mut info = info;
        info.runtime.max_tool_output_lines = info.runtime.max_tool_output_lines.max(1);
        let status = info
            .services
            .auth_unavailable
            .as_ref()
            .map(|_| "no providers configured; run /login to sign in".into())
            .unwrap_or_else(|| "ready".into());
        let pending_update_notice = info.services.pending_update_notice.take();
        let statusline = StatusLine::new(&info.runtime);
        Self {
            info,
            terminal_session: None,
            statusline,
            subagent_panel: SubagentPanel::default(),
            input_ui: InputUi::default(),
            status,
            should_quit: false,
            ctrl_c_streak: 0,
            streams: StreamUi::default(),
            turn: TurnUi::default(),
            image_picker: picker_from_environment(herdr_graphics),
            pending: PendingWorkUi::default(),
            pending_inline_shells: Vec::new(),
            deferred_inline_shell_context: Vec::new(),
            goal: None,
            history: HistoryUi::default(),
            credential_store,
            available_auths,
            using_unavailable_provider,
            pending_oauth_login: None,
            pending_usage_limits: None,
            usage_limits_client: reqwest::Client::new(),
            usage: UsageUi::default(),
            model_metadata: None,
            pending_model_metadata: None,
            pending_model_metadata_reasoning: None,
            pending_update_notice,
            pending_model_selection: None,
            internal_agent_model_target: None,
            pending_session_title: None,
            clipboard: Box::new(SystemClipboard::default()),
            last_mouse_position: None,
        }
    }
}
