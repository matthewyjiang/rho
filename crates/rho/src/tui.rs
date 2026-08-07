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

use history_cache::CachedCodeBlock;
use questionnaire::QuestionnaireCancelReason;
use ratatui::DefaultTerminal;
use tokio::sync::oneshot;
mod activity;
mod advisor_command;
mod advisor_status;
mod agent_editor;
mod agent_picker;
mod app_construct;
mod app_state;
mod approval;
pub(crate) mod attachment;
mod background_polls;
mod clipboard;
mod command_actions;
mod command_block;
mod command_palette;
mod compaction_display;
mod composer;
mod composer_chrome;
mod config_actions;
mod config_editor;
mod config_input;
mod config_picker;
mod context_handoff;
mod copy_interaction;
mod doctor;
pub(crate) mod event_adapter;
mod external_editor;
mod fast_command;
mod feed_image;
mod file_palette;
mod file_picker;
mod first_run;
mod frame_scheduler;
mod goal;
mod line_editor;
mod subagent_questionnaires;
mod text_input;
pub(crate) use first_run::SetupEntry;
pub(crate) use goal::GOAL_JUDGE_PROMPT;
mod changelog_command;
mod chat_media;
mod choice_actions;
mod claude_login;
mod composer_layout;
mod during_turn;
mod goal_command;
mod help_picker;
mod history_cache;
mod hook_actions;
mod info_command;
mod inline_choice;
mod inline_shell;
mod keybindings;
mod keyboard_modes;
mod limits_command;
mod local_commands;
mod local_diff;
mod login;
mod login_secret_input;
mod markdown;
mod markdown_image;
mod mcp_actions;
mod mcp_picker;
mod message_history;
mod message_render;
mod model_actions;
mod model_performance;
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
mod provider_actions;
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
mod setup_screen;
pub(in crate::tui) mod terminal_graph;
mod transcript_events;
pub(crate) use session_title::SESSION_TITLE_PROMPT;
mod app_loop;
mod idle_input;
mod reasoning_phase;
mod rewind_actions;
mod skill_actions;
mod skill_picker;
#[cfg(debug_assertions)]
mod smoke_injection;
mod status_overlay;
mod statusline;
mod stream;
mod stream_pace;
mod stream_preview;
mod subagent_attach;
mod subagent_panel;
mod terminal_events;
mod terminal_session;
mod text_selection;
mod theme;
mod tool_call_batch;
mod tool_card_render;
mod tool_diff;
mod tool_output_ui;
mod tree_actions;
mod turn_prompt;
mod usage_cost;
mod view;
mod view_composer;
mod workflow_discover;
mod workflow_hub;
// Separate full-screen mode for an active workflow run. The chat hub hands off
// through terminal suspend when starting or resuming a run.
pub(crate) mod workflow;
mod workspace;

mod types;
use types::*;

use activity::{ActivityPhase, ActivityStatus, LoadingSpinner};
use app_state::{HistoryUi, InputUi, PendingWorkUi, TurnUi};
use approval::{approval_lines, ApprovalKeyOutcome};
use chat_media::{ChatMedia, ChatTextDocument, ComposerAttachment, MediaAttachId};
use clipboard::ClipboardWriter;
use config_editor::{
    config_number_input_lines, resolve_web_search_editor_value, ConfigMutation, ConfigNumberInput,
    ConfigNumberKey, ConfigTextKey, ConfigToggle,
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
#[cfg(test)]
use inline_shell::InlineShellMode;
use login::PendingInteractiveLogin;
#[cfg(test)]
use login::SecretInput;
use paste_burst::PasteBurstEnter;
use picker::{
    sort_items_by_ascii_label, PickerAction, PickerBadge, PickerBadgePlacement, PickerBadgeTone,
    PickerItem, PickerKeyHints, PickerLayout, UiPicker,
};
use prompt_turn::FailedTurn;
#[cfg(test)]
use questionnaire::QuestionnaireComposer;
use questionnaire::{
    questionnaire_cursor_position, questionnaire_lines, questionnaire_notice_text,
    QuestionAnswerRequest, QuestionnaireReply, QuestionnaireResponseChannel,
};
use render::{
    char_prefix_display_width, display_width, input_cursor_position, input_label_lines,
    input_lines, labeled_divider_line, picker_lines, session_header_lines, styled_line,
    tool_entry_lines, truncate_one_line, LineFill,
};
use scrollbar::HistoryScrollbar;
use session_title::PendingSessionTitle;
use statusline::{GoalStatus, StatusLine};
use subagent_attach::PendingSubagentAttach;
use subagent_panel::SubagentPanel;
use terminal_session::TerminalSession;
use text_selection::{highlight_selection, render_copy_notice, TextSelection};
use theme::Theme;
use turn_prompt::TurnPrompt;

#[cfg(test)]
use rho_providers::model::{ImageContent, ModelUsage};
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
        ContentBlock, Message, ModelMetadata, ReasoningRequestSource, UnavailableProvider,
    },
    rho_providers::provider,
    rho_providers::reasoning::ReasoningLevel,
};
/// Viewport height used by line-level tests that render without a real terminal.
#[cfg(test)]
const DEFAULT_TUI_HEIGHT: u16 = 18;
const MAX_COMMAND_SUGGESTIONS: usize = 5;
const MIN_COMMAND_DESCRIPTION_WIDTH: usize = 7;
const RECOVERED_HISTORY_LINE_LIMIT: usize = 200;
/// Shared cadence for releasing held stream text and refreshing partial previews.
const STREAM_UI_TICK: Duration = Duration::from_millis(24);
const STREAM_PREVIEW_MIN_CHARS: usize = 2;
const HISTORY_SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(1200);
const HISTORY_MOUSE_SCROLL_LINES: usize = 3;
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
    pub service_tier: Option<rho_sdk::model::ServiceTier>,
    pub reasoning_source: ReasoningRequestSource,
    pub permission_mode: PermissionMode,
    pub show_reasoning_output: bool,
    pub zen_mode: bool,
    /// Offer the advisor tool, backed by the `advisor` internal agent's model.
    pub advisor_mode: bool,
    pub auth: String,
    pub internal_agents:
        std::collections::BTreeMap<String, crate::config::InternalAgentModelConfig>,
    pub favorite_models: Vec<String>,
    pub max_tool_output_lines: usize,
    pub keybindings: Keybindings,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
}

/// How reasoning appears in the transcript for the current display settings.
///
/// Zen and `show_reasoning_output` collapse into one exclusive policy so call
/// sites never invert complementary booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasoningChrome {
    /// Stream and store reasoning text in the transcript.
    FullText,
    /// Suppress reasoning text; show live `Thinking...` while the stretch is open.
    ThinkingPlaceholder,
    /// Suppress reasoning text and `Thinking...` (zen mode).
    Hidden,
}

impl RuntimeModelView {
    fn model_call_profile(&self) -> rho_sdk::ModelCallProfile {
        rho_sdk::ModelCallProfile {
            provider: self.provider.clone(),
            model: self.model.clone(),
            reasoning: self.reasoning,
            service_tier: self.service_tier,
        }
    }

    /// Exclusive reasoning display policy for the current session settings.
    pub(crate) fn reasoning_chrome(&self) -> ReasoningChrome {
        if self.zen_mode {
            ReasoningChrome::Hidden
        } else if self.show_reasoning_output {
            ReasoningChrome::FullText
        } else {
            ReasoningChrome::ThinkingPlaceholder
        }
    }

    /// Whether the TUI should render reasoning text for this session.
    pub(crate) fn displays_reasoning_output(&self) -> bool {
        matches!(self.reasoning_chrome(), ReasoningChrome::FullText)
    }

    /// Whether tool cards and reasoning blocks are visible in the transcript.
    ///
    /// Zen mode suppresses that work chrome while keeping the live activity rail
    /// and subagent rows so the session still shows progress. Reasoning text vs
    /// `Thinking...` vs neither is [`Self::reasoning_chrome`].
    pub(crate) fn shows_work_chrome(&self) -> bool {
        !self.zen_mode
    }

    pub(crate) fn history_render_settings(
        &self,
        width: usize,
    ) -> history_cache::HistoryRenderSettings {
        history_cache::HistoryRenderSettings {
            width,
            max_tool_output_lines: self.max_tool_output_lines,
            zen_mode: self.zen_mode,
        }
    }

    fn fast_mode_active(&self) -> bool {
        self.service_tier == Some(rho_sdk::model::ServiceTier::Priority)
            && rho_providers::providers::openai::supports_fast_mode(&self.provider, &self.model)
    }
}

pub struct SessionBootstrap {
    pub session_id: Option<String>,
    pub recovered_messages: Vec<Message>,
    pub open_resume_picker: bool,
}

pub struct ApplicationServices {
    pub(crate) config_repository: ConfigRepository,
    /// Set when this launch should open the first-run setup screen, and at
    /// which step. `None` for a returning session.
    pub(crate) first_run: Option<first_run::SetupEntry>,
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
pub(crate) use attachment::{run as run_attachment, translate_run_event};

pub async fn run(agent: &mut InteractiveRuntime, info: TuiBootstrap) -> anyhow::Result<TuiResult> {
    let mut terminal = ratatui::init();
    Theme::initialize_from_terminal();
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
                let mut app = App::new(info, herdr_graphics, agent.mcp_report().clone());
                app.terminal_session = Some(TerminalSession::acquire());
                if let Some(manager) = agent.subagents() {
                    app.subagent_host_input = Some(manager.bind_host_input());
                }
                let result = app.run(&mut terminal, agent).await;
                if let Some(manager) = agent.subagents() {
                    manager.unbind_host_input();
                }
                result
            }
            Err(error) => Err(error),
        }
    };
    herdr.release().await;
    ratatui::restore();
    if let Ok(result) = &result {
        app_loop::print_exit_summary(result.exit_summary.as_deref())?;
    }
    result
}

struct App {
    info: TuiBootstrap,
    terminal_session: Option<TerminalSession>,
    statusline: StatusLine,
    subagent_panel: SubagentPanel,
    subagent_host_input: Option<
        tokio::sync::mpsc::Receiver<crate::app::subagent_host_input::SubagentHostInputRequest>,
    >,
    queued_subagent_questionnaires:
        VecDeque<crate::app::subagent_host_input::SubagentHostInputRequest>,
    pending_subagent_questionnaire: Option<PendingSubagentQuestionnaire>,
    input_ui: InputUi,
    /// Tiny disappearing feedback toast. Write only through [`App::set_status`]
    /// / [`App::notify_status`].
    status_overlay: Option<status_overlay::StatusOverlay>,
    /// Last status text for callers that inspect mode feedback.
    last_status: String,
    should_quit: bool,
    ctrl_c_streak: u8,
    streams: StreamUi,
    turn: TurnUi,
    image_picker: Option<ratatui_image::picker::Picker>,
    pending: PendingWorkUi,
    pending_inline_shells: Vec<inline_shell::PendingShellTask>,
    deferred_inline_shell_context: Vec<inline_shell::DeferredShellContext>,
    goal: Option<GoalState>,
    history: HistoryUi,
    credential_store: Arc<dyn CredentialStore>,
    available_auths: Vec<String>,
    using_unavailable_provider: bool,
    pending_interactive_login: Option<PendingInteractiveLogin>,
    /// Active step of the first-launch setup screen, or `None` for a normal
    /// session. While set, the screen replaces all session chrome.
    setup_screen: Option<setup_screen::SetupStep>,
    pending_usage_limits: Option<tokio::task::JoinHandle<limits_command::LimitsFetchResult>>,
    pending_changelog: Option<tokio::task::JoinHandle<changelog_command::ChangelogFetchResult>>,
    usage_limits_client: reqwest::Client,
    usage: UsageUi,
    model_metadata: Option<ModelMetadata>,
    pending_model_metadata: Option<tokio::task::JoinHandle<Option<ModelMetadata>>>,
    pending_model_metadata_reasoning: Option<(ReasoningLevel, ReasoningRequestSource)>,
    pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pending_model_selection: Option<InteractiveModelSelection>,
    internal_agent_model_target: Option<agent_picker::InternalAgentModelTarget>,
    agent_editor_session: Option<agent_editor::AgentEditSession>,
    pending_session_title: Option<PendingSessionTitle>,
    /// Set by `/title` so auto-title generation cannot overwrite a manual name.
    session_title_locked: bool,
    clipboard: Box<dyn ClipboardWriter + Send>,
    media_attach_tasks: Vec<clipboard::MediaAttachTask>,
    pending_subagent_attaches: Vec<PendingSubagentAttach>,
    last_mouse_position: Option<(u16, u16)>,
    /// Screen-space drag selection for text outside the history area.
    screen_selection: Option<TextSelection>,
    /// MCP inventory for `/mcp` and `/doctor` (session snapshot from tool assembly).
    mcp_report: crate::tools::mcp::McpSessionReport,
}

struct PendingSubagentQuestionnaire {
    run_id: String,
    agent_id: String,
    reply_rx: oneshot::Receiver<QuestionnaireReply>,
    response_tx: tokio::sync::oneshot::Sender<Result<rho_sdk::HostInputResponse, rho_sdk::Error>>,
}

#[cfg(test)]
#[path = "tui/app_tests.rs"]
mod tests;
