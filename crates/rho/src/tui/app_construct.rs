//! App construction helpers for the interactive TUI.

use std::collections::VecDeque;
use std::sync::Arc;

use rho_providers::credentials::{available_auth_modes, CredentialStore};

use crate::credential_store::AppCredentialStore;

use super::{
    activity::{ActivityPhase, LoadingSpinner},
    clipboard::SystemClipboard,
    feed_image::picker_from_environment,
    history_cache::HistoryLineCache,
    markdown_image,
    paste_burst::PasteBurst,
    pending_input::PendingInputPanel,
    provider_attempt::ProviderAttempt,
    reasoning_phase,
    statusline::StatusLine,
    subagent_panel::SubagentPanel,
    tool_call_batch::ToolCallBatch,
    App, ComposerMode, HistoryScroll, InputSubmissionMode, StreamUi, TuiBootstrap, UsageUi,
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
            terminal_events: None,
            statusline,
            subagent_panel: SubagentPanel::default(),
            input: String::new(),
            input_cursor: 0,
            shell_mode: None,
            status,
            should_quit: false,
            ctrl_c_streak: 0,
            streams: StreamUi::default(),
            current_turn_start: None,
            provider_attempt: ProviderAttempt::default(),
            reasoning_phase: reasoning_phase::ReasoningPhase::default(),
            session_ui: Default::default(),
            activity_phase: ActivityPhase::default(),
            loading_spinner: LoadingSpinner::default(),
            tool_calls: ToolCallBatch::default(),
            image_picker: picker_from_environment(herdr_graphics),
            steering_prompts: VecDeque::new(),
            accepted_steering: VecDeque::new(),
            retracting_steering: None,
            pending_input_panel: PendingInputPanel::default(),
            pending_input_action: None,
            queued_prompts: VecDeque::new(),
            pending_inline_shells: Vec::new(),
            deferred_inline_shell_context: Vec::new(),
            goal: None,
            pending_images: Vec::new(),
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: None,
            paste_burst: PasteBurst::default(),
            paste_segments: Vec::new(),
            input_submission_mode: InputSubmissionMode::default(),
            transcript: Vec::new(),
            history_lines: HistoryLineCache::default(),
            last_status_notice: None,
            last_inserted_was_tool: false,
            command_selection: 0,
            command_prefix: None,
            command_palette_dismissed: false,
            file_selection: 0,
            file_query: None,
            file_palette_dismissed: false,
            file_match_cache: None,
            skill_match_cache: None,
            composer: ComposerMode::Input,
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
            markdown_images: markdown_image::MarkdownImageCache::default(),
            markdown_images_dirty_from: None,
            history_scroll: HistoryScroll::Bottom,
            history_scrollbar_drag: None,
            history_scrollbar_visible_until: None,
            history_scrollbar_hovered: false,
            hovered_code_block_copy: None,
            text_selection: None,
            copy_notice: None,
            clipboard: Box::new(SystemClipboard::default()),
            session_header_cache: None,
            last_mouse_position: None,
        }
    }
}
