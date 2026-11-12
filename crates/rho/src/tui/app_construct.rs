//! App construction helpers for the interactive TUI.

use std::sync::Arc;

use rho_providers::credentials::CredentialStore;

use crate::credential_store::AppCredentialStore;

use super::{
    app_state::{HistoryUi, InputUi, PendingWorkUi, TurnUi},
    clipboard::SystemClipboard,
    feed_image::picker_from_environment,
    palette::PaletteCaches,
    process_panel::ProcessPanel,
    statusline::StatusLine,
    subagent_panel::SubagentPanel,
    workflow_panel::WorkflowPanel,
    App, StatusSource, StreamUi, TuiBootstrap, UsageUi,
};

#[cfg(debug_assertions)]
use super::smoke_injection;

impl App {
    pub(super) fn new(
        info: TuiBootstrap,
        program_status_support: super::program_status::ProgramStatusSupport,
    ) -> Self {
        // Matrix mode is debug-only (matrix_enabled is always false in release).
        #[cfg(debug_assertions)]
        if smoke_injection::matrix_enabled() {
            smoke_injection::seed_matrix_model_cache();
        }
        Self::new_with_credentials(info, Arc::new(AppCredentialStore), program_status_support)
    }

    pub(super) fn new_with_credentials(
        info: TuiBootstrap,
        credential_store: Arc<dyn CredentialStore>,
        program_status_support: super::program_status::ProgramStatusSupport,
    ) -> Self {
        // Pickers call `refresh_available_auths()` on open. Sweeping the
        // keyring here would stall the first frame on D-Bus.
        let available_auths = Vec::new();
        let mut info = info;
        info.runtime.max_tool_output_lines = info.runtime.max_tool_output_lines.max(1);
        let initial_status = info
            .services
            .auth_unavailable
            .as_ref()
            .map(|_| "no providers configured; run /login to sign in".to_string());
        let tasks = super::background_tasks::BackgroundTasks::from_startup(&mut info.services);
        let output_streaming = info.services.settings.output_streaming;
        let prompt_history = super::prompt_history::PromptHistory::new(
            info.services.settings.prompt_history_limit,
            info.services.pending_prompt_history.take(),
        );
        let notifier = super::notifications::TerminalNotifier::new(
            super::notifications::NotificationChannel::detect(|key| std::env::var(key).ok()),
        );
        let mut statusline = StatusLine::new(&info.runtime);
        statusline.update_not_saved(info.session.no_save);
        let mut app = Self {
            info,
            terminal_session: None,
            statusline,
            subagent_panel: SubagentPanel::default(),
            process_panel: ProcessPanel::default(),
            workflow_panel: WorkflowPanel::default(),
            daemon_answers: Default::default(),
            input_ui: InputUi::default(),
            palette_caches: PaletteCaches::default(),
            status_overlay: None,
            last_status: String::new(),
            status_source: StatusSource::default(),
            should_quit: false,
            ctrl_c_streak: 0,
            turn_finished_attention: false,
            streams: StreamUi {
                mode: output_streaming,
                ..StreamUi::default()
            },
            turn: TurnUi::default(),
            image_picker: picker_from_environment(),
            pending: PendingWorkUi::default(),
            pending_inline_shells: Vec::new(),
            history: HistoryUi::default(),
            credential_store,
            available_auths,
            exclusive: super::exclusive_screen::ExclusiveOccupant::Session,
            tasks,
            info_tree_deferred: false,
            spend: Default::default(),
            usage_limits_live: std::collections::BTreeMap::new(),
            diff_viewer: None,
            usage_limits_client: std::sync::OnceLock::new(),
            usage: UsageUi::default(),
            model_metadata: None,
            prompt_history,
            held_turns: std::collections::VecDeque::new(),
            start_follow_ups: false,
            acknowledged_send_omissions: None,
            model_picker_scope_override: None,
            internal_agent_model_target: None,
            pending_auto_classifier_demote: false,
            agent_editor_session: None,
            sessions_hub_state: super::sessions_hub::SessionsHubState::default(),
            pending_sessions_task: None,
            jobs_view_attach: None,
            daemon_options: None,
            daemon_option_requests: std::collections::VecDeque::new(),
            daemon_login_selected: None,
            daemon_unshown_switch: None,
            daemon_title: None,
            daemon_compact_turn: None,
            daemon_compact_handoff: false,
            info_compaction_read: Default::default(),
            awaited_daemon_report: None,
            daemon_rail: Default::default(),
            daemon_login: None,
            daemon_settings: None,
            todo_list: None,
            clipboard: Box::new(SystemClipboard::default()),
            media_attach_tasks: Vec::new(),
            composer_attachment_layout_cache: None,

            attach_run_filter: super::attach_picker::WorkspaceRunFilter::RunningOnly,
            attach_disk_candidates: Vec::new(),
            attach_seen_live: std::collections::HashSet::new(),
            last_mouse_position: None,
            screen_selection: None,
            mcp_argument_completions:
                super::mcp_argument_completion::McpArgumentCompletions::default(),
            daemon_mcp_offers: super::mcp_source::DaemonMcpOffers::default(),
            side_chat: None,
            herdr_sync: Default::default(),
            notifier,
            program_status: super::program_status::ProgramStatusReporter::new(
                program_status_support,
            ),
            settled_program_status: super::program_status::ProgramStatus::Idle,
        };
        if let Some(status) = initial_status {
            app.set_status(status);
        }
        app
    }
}
