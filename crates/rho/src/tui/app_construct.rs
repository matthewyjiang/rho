//! App construction helpers for the interactive TUI.

use std::sync::Arc;

use rho_providers::credentials::CredentialStore;

use crate::credential_store::AppCredentialStore;

use super::{
    app_state::{HistoryUi, InputUi, PendingWorkUi, TurnUi},
    clipboard::SystemClipboard,
    feed_image::picker_from_environment,
    process_panel::ProcessPanel,
    statusline::StatusLine,
    subagent_panel::SubagentPanel,
    App, StatusSource, StreamUi, TuiBootstrap, UsageUi,
};

#[cfg(debug_assertions)]
use super::smoke_injection;

impl App {
    pub(super) fn new(
        info: TuiBootstrap,
        herdr_graphics: crate::herdr::HerdrGraphicsCapability,
        mcp_report: crate::tools::mcp::McpSessionReport,
        mcp_catalog: crate::tools::mcp::McpCatalog,
        plugins_report: crate::plugins::PluginLoadReport,
    ) -> Self {
        // Matrix mode is debug-only (matrix_enabled is always false in release).
        #[cfg(debug_assertions)]
        if smoke_injection::matrix_enabled() {
            smoke_injection::seed_matrix_model_cache();
            return Self::new_with_credentials(
                info,
                Arc::new(rho_providers::credentials::MemoryCredentialStore::default()),
                herdr_graphics,
                mcp_report,
                mcp_catalog,
                plugins_report,
            );
        }
        Self::new_with_credentials(
            info,
            Arc::new(AppCredentialStore),
            herdr_graphics,
            mcp_report,
            mcp_catalog,
            plugins_report,
        )
    }

    pub(super) fn new_with_credentials(
        info: TuiBootstrap,
        credential_store: Arc<dyn CredentialStore>,
        herdr_graphics: crate::herdr::HerdrGraphicsCapability,
        mcp_report: crate::tools::mcp::McpSessionReport,
        mcp_catalog: crate::tools::mcp::McpCatalog,
        plugins_report: crate::plugins::PluginLoadReport,
    ) -> Self {
        // Pickers call `refresh_available_auths()` on open. Sweeping the
        // keyring here would stall the first frame on D-Bus.
        let available_auths = Vec::new();
        let using_unavailable_provider = info.services.auth_unavailable.is_some();
        let mut info = info;
        info.runtime.max_tool_output_lines = info.runtime.max_tool_output_lines.max(1);
        let initial_status = info
            .services
            .auth_unavailable
            .as_ref()
            .map(|_| "no providers configured; run /login to sign in".to_string());
        let pending_update_notice = info.services.pending_update_notice.take();
        let pending_custom_models = info.services.pending_custom_models.take();
        let statusline = StatusLine::new(&info.runtime);
        let mut app = Self {
            info,
            terminal_session: None,
            statusline,
            subagent_panel: SubagentPanel::default(),
            process_panel: ProcessPanel::default(),
            subagent_inbox: crate::tui::subagent_inbox::SubagentInbox::default(),
            pending_subagent_questionnaire: None,
            input_ui: InputUi::default(),
            status_overlay: None,
            last_status: String::new(),
            status_source: StatusSource::default(),
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
            pending_interactive_login: None,
            setup_screen: None,
            pending_usage_limits: Vec::new(),
            usage_limits_live: std::collections::BTreeMap::new(),
            pending_changelog: None,
            usage_limits_client: std::sync::OnceLock::new(),
            usage: UsageUi::default(),
            model_metadata: None,
            pending_model_metadata: None,
            pending_model_metadata_reasoning: None,
            pending_update_notice,
            pending_custom_models,
            pending_herdr_graphics: None,
            held_turns: std::collections::VecDeque::new(),
            compact_follow_up: super::compact_work::CompactFollowUp::None,
            start_follow_ups: None,
            pending_model_selection: None,
            internal_agent_model_target: None,
            pending_auto_classifier_demote: false,
            agent_editor_session: None,
            sessions_hub_state: super::sessions_hub::SessionsHubState::default(),
            pending_session_title: None,
            session_title_locked: false,
            clipboard: Box::new(SystemClipboard::default()),
            media_attach_tasks: Vec::new(),
            terminal_height: 0,
            composer_attachment_layout_cache: None,
            pending_subagent_attaches: Vec::new(),
            attach_run_filter: super::attach_picker::WorkspaceRunFilter::RunningOnly,
            attach_disk_candidates: Vec::new(),
            attach_seen_live: std::collections::HashSet::new(),
            last_mouse_position: None,
            screen_selection: None,
            mcp_report,
            mcp_catalog,
            mcp_argument_completions:
                super::mcp_argument_completion::McpArgumentCompletions::default(),
            plugins_report,
        };
        if let Some(status) = initial_status {
            app.set_status(status);
        }
        app
    }
}
