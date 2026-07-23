use std::{
    collections::VecDeque,
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use history_cache::{CachedCodeBlock, HistoryLineCache};
use questionnaire::QuestionnaireCancelReason;
#[cfg(test)]
use std::sync::Mutex;
use tokio::sync::oneshot;
use tool_call_batch::ToolCallBatch;

use ratatui::DefaultTerminal;
#[cfg(test)]
use ratatui::{layout::Rect, style::Modifier, text::Line};
mod activity;
mod agent_picker;
mod app_construct;
mod approval;
mod attachment;
mod background_polls;
mod clipboard;
mod command_actions;
mod command_block;
mod command_palette;
mod composer;
mod config_actions;
mod config_editor;
mod config_input;
mod config_picker;
mod context_handoff;
mod copy_interaction;
mod doctor;
mod event_adapter;
mod feed_image;
mod file_palette;
mod file_picker;
mod frame_scheduler;
mod goal;
pub(crate) use goal::GOAL_JUDGE_PROMPT;
mod choice_actions;
mod during_turn;
mod goal_command;
mod help_picker;
mod history_cache;
mod info_command;
mod inline_choice;
mod inline_shell;
mod keybindings;
mod keyboard_modes;
mod limits_command;
mod local_commands;
mod local_diff;
mod login;
mod markdown;
mod markdown_image;
mod message_history;
mod message_render;
mod model_actions;
mod model_picker;
mod mouse;
mod mouse_capture;
mod paste_burst;
mod pending_input;
mod permission_mode;
mod picker;
mod picker_input;
mod picker_overlay;
mod prompt_turn;
mod provider_attempt;
mod provider_picker;
mod questionnaire;
mod questionnaire_input;
mod reasoning_metadata;
mod render;
mod rendered_entry;
mod run_lifecycle;
mod screen_layout;
mod scrollbar;
mod session_actions;
mod session_picker;
mod session_title;
mod transcript_events;
pub(crate) use session_title::SESSION_TITLE_PROMPT;
mod app_loop;
mod idle_input;
mod reasoning_phase;
mod skill_actions;
mod skill_picker;
#[cfg(debug_assertions)]
mod smoke_injection;
mod statusline;
mod stream;
mod stream_preview;
mod subagent_panel;
mod terminal_events;
mod text_selection;
mod theme;
mod tool_call_batch;
mod tool_diff;
mod tool_output_ui;
mod tree_actions;
mod turn_prompt;
mod usage_cost;
mod view;
mod workspace;

mod types;
use types::*;

use activity::{ActivityPhase, ActivityStatus, LoadingSpinner};
use approval::{approval_lines, ApprovalKeyOutcome};
use clipboard::ClipboardWriter;
use config_editor::{
    config_number_input_lines, config_text_input_lines, resolve_web_search_editor_value,
    ConfigMutation, ConfigNumberInput, ConfigNumberKey, ConfigTextInput, ConfigTextKey,
    ConfigToggle,
};
use copy_interaction::CodeBlockCopyTarget;
use event_adapter::{SdkEventAdapter, ViewEvent, ViewModelEvent};
use feed_image::FeedImage;
use frame_scheduler::FrameScheduler;
use goal::GoalState;
use inline_choice::{
    InlineChoice, InlineChoiceKeyOutcome, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending,
};
use inline_shell::InlineShellMode;
use login::PendingOAuthLogin;
#[cfg(test)]
use login::SecretInput;
use paste_burst::{PasteBurst, PasteBurstEnter};
use pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel};
use picker::{
    sort_items_by_ascii_label, PickerAction, PickerBadge, PickerBadgePlacement, PickerBadgeTone,
    PickerItem, PickerLayout, UiPicker,
};
use prompt_turn::FailedTurn;
use provider_attempt::ProviderAttempt;
#[cfg(test)]
use questionnaire::QuestionnaireComposer;
use questionnaire::{
    questionnaire_cursor_position, questionnaire_lines, questionnaire_notice_text,
    QuestionAnswerRequest, QuestionnaireReply, QuestionnaireResponseChannel,
};
use render::{
    char_prefix_display_width, display_width, input_cursor_position, input_lines_with_images,
    labeled_divider_line, picker_lines, session_header_lines, styled_line, tool_entry_lines,
    truncate_one_line, LineFill,
};
use scrollbar::{scroll_state_for_top_line, HistoryScrollbar, HistoryScrollbarDrag};
use session_title::PendingSessionTitle;
use statusline::{GoalStatus, StatusLine};
use subagent_panel::SubagentPanel;
use terminal_events::TerminalEvents;
use text_selection::{highlight_selection, render_copy_notice, CopyNotice, TextSelection};
use theme::Theme;
use turn_prompt::TurnPrompt;

use {
    crate::app::config_repository::ConfigRepository,
    crate::app::interactive_runtime::InteractiveRuntime,
    crate::commands::{self, CommandId, CommandInvocation},
    crate::herdr::{HerdrReporter, HerdrState},
    crate::keybindings::Keybindings,
    crate::permission::PermissionMode,
    crate::session::Session,
    rho_providers::credentials::CredentialStore,
    rho_providers::model::{
        catalog::{self, LoginTarget, ModelSelection},
        favorites,
        provider_models::refresh_provider_models_with_store,
        ContentBlock, ImageContent, Message, ModelMetadata, ReasoningRequestSource,
        UnavailableProvider,
    },
    rho_providers::provider,
    rho_providers::reasoning::ReasoningLevel,
};
#[cfg(test)]
use {rho_providers::model::ModelUsage, rho_tools::tool::ToolDisplayStyle};
const DEFAULT_TUI_HEIGHT: u16 = 18;
const PASTE_COLLAPSE_MIN_LINES: usize = 2;
const PASTE_COLLAPSE_MIN_CHARS: usize = 1000;
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const MIN_COMMAND_DESCRIPTION_WIDTH: usize = 7;
const MAX_TERMINAL_EVENTS_PER_TICK: usize = 256;
const RECOVERED_HISTORY_LINE_LIMIT: usize = 200;
const STREAM_PREVIEW_DELAY: Duration = Duration::from_millis(24);
const STREAM_PREVIEW_MIN_CHARS: usize = 2;
const HISTORY_SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(1200);
pub struct TuiBootstrap {
    pub runtime: RuntimeModelView,
    pub session: SessionBootstrap,
    pub services: ApplicationServices,
}

pub struct RuntimeModelView {
    pub cwd: PathBuf,
    pub provider: String,
    pub model: String,
    pub(crate) model_aliases: crate::model_aliases::ModelAliases,
    pub reasoning: ReasoningLevel,
    pub reasoning_source: ReasoningRequestSource,
    pub permission_mode: PermissionMode,
    pub show_reasoning_output: bool,
    pub auth: String,
    pub internal_agents:
        std::collections::BTreeMap<String, crate::config::InternalAgentModelConfig>,
    pub favorite_models: Vec<String>,
    pub max_tool_output_lines: usize,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
}

pub struct SessionBootstrap {
    pub session_id: Option<String>,
    pub recovered_messages: Vec<Message>,
    pub open_resume_picker: bool,
}

pub struct ApplicationServices {
    pub(crate) config_repository: ConfigRepository,
    pub auth_unavailable: Option<String>,
    pub update_notice: Option<String>,
    pub pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pub diagnostics: crate::diagnostics::RuntimeDiagnostics,
    pub herdr: HerdrReporter,
}
pub struct TuiResult {
    pub resume_session_id: Option<String>,
    exit_summary: Option<String>,
}
pub(crate) use attachment::{run as run_attachment, AttachmentWriter};

pub async fn run(agent: &mut InteractiveRuntime, info: TuiBootstrap) -> anyhow::Result<TuiResult> {
    let mut terminal = ratatui::init();
    Theme::initialize_from_terminal();
    let keyboard = keyboard_modes::Enabled::acquire();
    let mouse_capture_enabled = mouse_capture::enable().is_ok();
    let herdr = info.services.herdr.clone();
    let herdr_graphics = herdr.graphics_capability().await;
    let initial_state = if info.services.auth_unavailable.is_some() {
        HerdrState::Blocked
    } else {
        HerdrState::Idle
    };
    herdr
        .report_state(
            initial_state,
            info.services.auth_unavailable.as_deref(),
            info.session.session_id.as_deref(),
        )
        .await;
    let result = {
        #[cfg(debug_assertions)]
        let injected = smoke_injection::after_terminal_init();
        #[cfg(not(debug_assertions))]
        let injected: anyhow::Result<()> = Ok(());

        match injected {
            Ok(()) => {
                App::new(info, herdr_graphics)
                    .run(&mut terminal, agent)
                    .await
            }
            Err(error) => Err(error),
        }
    };
    herdr.release().await;
    keyboard.release();
    if mouse_capture_enabled {
        let _ = mouse_capture::disable();
    }
    ratatui::restore();
    if let Ok(result) = &result {
        app_loop::print_exit_summary(result.exit_summary.as_deref())?;
    }
    result
}

struct App {
    info: TuiBootstrap,
    terminal_events: Option<TerminalEvents>,
    statusline: StatusLine,
    subagent_panel: SubagentPanel,
    input: String,
    input_cursor: usize,
    /// Explicit shell-mode state. Composer text stores only the command body.
    shell_mode: Option<InlineShellMode>,
    status: String,
    should_quit: bool,
    ctrl_c_streak: u8,
    streams: StreamUi,
    current_turn_start: Option<usize>,
    provider_attempt: ProviderAttempt,
    reasoning_phase: reasoning_phase::ReasoningPhase,
    session_ui: run_lifecycle::SessionUiPhase,
    activity_phase: ActivityPhase,
    loading_spinner: LoadingSpinner,
    tool_calls: ToolCallBatch,
    image_picker: Option<ratatui_image::picker::Picker>,
    steering_prompts: VecDeque<QueuedPrompt>,
    accepted_steering: VecDeque<AcceptedSteering>,
    retracting_steering: Option<rho_sdk::SteeringId>,
    pending_input_panel: PendingInputPanel,
    pending_input_action: Option<PendingInputAction>,
    queued_prompts: VecDeque<QueuedPrompt>,
    pending_inline_shells: Vec<inline_shell::PendingShellTask>,
    deferred_inline_shell_context: Vec<inline_shell::DeferredShellContext>,
    goal: Option<GoalState>,
    pending_images: Vec<ImageContent>,
    input_history: Vec<String>,
    input_history_cursor: Option<usize>,
    input_history_draft: Option<InputDraft>,
    paste_burst: PasteBurst,
    paste_segments: Vec<PasteSegment>,
    input_submission_mode: InputSubmissionMode,
    transcript: Vec<Entry>,
    history_lines: HistoryLineCache,
    last_status_notice: Option<String>,
    last_inserted_was_tool: bool,
    command_selection: usize,
    command_prefix: Option<String>,
    command_palette_dismissed: bool,
    file_selection: usize,
    file_query: Option<String>,
    file_palette_dismissed: bool,
    file_match_cache: Option<FileMatchCache>,
    skill_match_cache: Option<SkillMatchCache>,
    composer: ComposerMode,
    credential_store: Arc<dyn CredentialStore>,
    available_auths: Vec<String>,
    using_unavailable_provider: bool,
    pending_oauth_login: Option<PendingOAuthLogin>,
    pending_usage_limits: Option<tokio::task::JoinHandle<limits_command::LimitsFetchResult>>,
    usage_limits_client: reqwest::Client,
    usage: UsageUi,
    model_metadata: Option<ModelMetadata>,
    pending_model_metadata: Option<tokio::task::JoinHandle<Option<ModelMetadata>>>,
    pending_model_metadata_reasoning: Option<(ReasoningLevel, ReasoningRequestSource)>,
    pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pending_model_selection: Option<InteractiveModelSelection>,
    internal_agent_model_target: Option<String>,
    pending_session_title: Option<PendingSessionTitle>,
    markdown_images: markdown_image::MarkdownImageCache,
    markdown_images_dirty_from: Option<usize>,
    history_scroll: HistoryScroll,
    history_scrollbar_drag: Option<HistoryScrollbarDrag>,
    history_scrollbar_visible_until: Option<Instant>,
    history_scrollbar_hovered: bool,
    hovered_code_block_copy: Option<usize>,
    text_selection: Option<TextSelection>,
    copy_notice: Option<CopyNotice>,
    clipboard: Box<dyn ClipboardWriter + Send>,
    session_header_cache: Option<SessionHeaderCache>,
    last_mouse_position: Option<(u16, u16)>,
}

#[cfg(test)]
#[path = "tui/app_tests.rs"]
mod tests;
