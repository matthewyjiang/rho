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

use questionnaire::QuestionnaireCancelReason;
use ratatui::DefaultTerminal;
use tokio::sync::oneshot;
use tracing::Instrument;
mod activity;
mod advisor_command;
mod advisor_status;
mod agent_creator_command;
mod agent_editor;
mod agent_picker;
mod agent_tools_picker;
mod app_construct;
mod app_state;
mod approval;
mod attach_picker;
pub(crate) mod attachment;
mod background_polls;
mod cache_stats;
mod clipboard;
mod command_actions;
mod command_block;
mod command_palette;
mod compact_work;
mod compaction_display;
mod composer;
mod composer_attachments;
mod composer_chrome;
mod config_actions;
mod config_editor;
mod config_input;
mod config_picker;
mod config_row;
mod context_handoff;
mod copy_interaction;
mod divider;
mod doctor_overlay;
pub(crate) mod event_adapter;
mod external_editor;
mod external_login;
mod fast_command;
mod feed_image;
mod file_palette;
mod file_picker;
mod first_run;
mod frame_context;
mod frame_scheduler;
mod goal;
mod line_editor;
mod subagent_inbox;
mod subagent_questionnaires;
mod text_input;

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

pub(crate) use first_run::SetupEntry;
pub(crate) use goal::GOAL_JUDGE_PROMPT;
mod changelog_command;
mod chat_media;
mod choice_actions;
mod claude_login;
mod composer_layout;
mod cursor_login;
mod cursor_model_picker;
mod custom_provider_login;
mod during_turn;
mod exclusive_screen;
mod exit_receipt;
mod goal_command;
mod help_picker;
mod history_cache;
mod history_soft_settings;
mod hook_actions;
mod info_command;
mod inline_choice;
mod inline_shell;
mod keybindings;
mod keyboard_modes;
mod limits_command;
mod linger_rail;
mod local_commands;
mod local_diff;
mod login;
mod login_presentation;
mod login_secret_input;
mod login_target;
mod markdown;
mod markdown_image;
mod mcp_actions;
mod mcp_argument_completion;
mod mcp_picker;
mod mcp_prompt;
mod mcp_resource;
mod media_attach;
mod message_history;
mod message_render;
mod model_actions;
mod model_cycle;
mod model_performance;
mod model_picker;
mod models_dev_actions;
mod mouse;
mod mouse_capture;
mod overlay_panel;
mod palette;
mod panel_text;
mod paste_burst;
mod pending_input;
#[cfg(test)]
mod performance_benchmarks;
mod permission_mode;
mod picker;
mod picker_actions;
mod process_panel;
mod process_peek;
mod prompt_history;
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
mod sessions_hub;
mod setup_screen;
mod side_chat;
mod syntax;
mod syntax_warmup;
pub(crate) use syntax_warmup::spawn_syntax_warmup;
pub(in crate::tui) mod terminal_graph;
mod transcript_events;
pub(crate) use session_title::SESSION_TITLE_PROMPT;
mod app_loop;
mod idle_input;
mod reasoning_phase;
mod rewind_actions;
mod skill_actions;
mod skill_picker;
mod startup_prompt;
// Always compiled: display_version() is used in release TUI chrome.
// Matrix/herdr injection paths stay no-ops outside debug builds.
mod github_pr;
mod smoke_injection;
mod status_overlay;
mod statusline;
pub(in crate::tui) use statusline::reasoning_is_configurable;
mod stream;
mod stream_pace;
mod stream_preview;
mod subagent_attach;
mod subagent_panel;
mod terminal_events;
mod terminal_session;
mod text_selection;
mod theme;
mod theme_actions;
mod theme_picker;
mod theme_scheme;
mod theme_terminal;
mod tool_call_batch;
mod tool_card_hover;
mod tool_card_render;
mod tool_diff;
mod tool_output_ui;
mod tool_search;
mod tree_actions;
mod turn_prompt;
mod usage_cost;
mod view;
mod view_composer;
mod view_scroll;
mod workflow_discover;
mod workflow_hub;
// Separate full-screen mode for an active workflow run. The chat hub hands off
// through terminal suspend when starting or resuming a run.
pub(crate) mod workflow;
mod workspace;

mod types;
use types::*;

use activity::{ActivityPhase, ActivityStatus, BackgroundCounts, LoadingSpinner};
use app_state::{HistoryUi, InputUi, PendingWorkUi, TurnUi};
use approval::{approval_lines, ApprovalKeyOutcome};
use chat_media::{
    ChatMedia, ChatTextDocument, ComposerAttachment, MediaAttachId, PendingAttachmentSource,
};
use clipboard::Clipboard;
use config_editor::{
    config_number_input_lines, resolve_web_search_editor_value, ConfigNumberInput, ConfigNumberKey,
    ConfigTextKey, ConfigToggle,
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
    sort_items_by_ascii_label, PickerBadge, PickerBadgePlacement, PickerBadgeTone, PickerCursor,
    PickerItem, PickerKeyHints, PickerLayout, UiPicker,
};
use process_panel::ProcessPanel;
use prompt_turn::FailedTurn;
use questionnaire::{
    questionnaire_cursor_position, questionnaire_lines, questionnaire_notice_text,
    QuestionAnswerRequest, QuestionnaireReply, QuestionnaireResponseChannel,
};
use render::{
    char_prefix_display_width, display_width, input_frame, picker_lines, session_header_lines,
    styled_line, tool_entry_lines, truncate_one_line, InputFrame, LineFill,
};
use scrollbar::HistoryScrollbar;
use session_title::PendingSessionTitle;
use statusline::{GoalStatus, StatusLine};
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
    crate::herdr::{HerdrGraphicsCapability, HerdrReporter, HerdrState},
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
/// Shared cadence for releasing held stream text and refreshing partial previews.
const STREAM_UI_TICK: Duration = Duration::from_millis(24);
const STREAM_PREVIEW_MIN_CHARS: usize = 2;
const HISTORY_SCROLLBAR_REVEAL_DURATION: Duration = Duration::from_millis(1200);
pub(in crate::tui) const HISTORY_MOUSE_SCROLL_LINES: usize = 3;
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
    /// Show a transcript notice after a turn that re-billed a large uncached prompt.
    pub cache_miss_notices: bool,
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
    /// and live subagent or process rows so the session still shows progress. Reasoning text vs
    /// `Thinking...` vs neither is [`Self::reasoning_chrome`].
    pub(crate) fn shows_work_chrome(&self) -> bool {
        !self.zen_mode
    }

    pub(crate) fn history_render_settings(
        &self,
        width: usize,
        max_image_height: u16,
    ) -> history_cache::HistoryRenderSettings {
        history_cache::HistoryRenderSettings {
            width,
            max_tool_output_lines: self.max_tool_output_lines,
            zen_mode: self.zen_mode,
            theme_generation: theme::Theme::generation(),
            max_image_height,
        }
    }

    fn fast_mode_active(&self) -> bool {
        self.service_tier == Some(rho_sdk::model::ServiceTier::Priority)
            && rho_providers::providers::openai::supports_fast_mode(&self.provider, &self.model)
    }
}

pub struct SessionBootstrap {
    pub session_id: Option<String>,
    /// Take-once startup buffer. `insert_recovered_history` converts this into
    /// transcript entries and drops the `Message` vec.
    pub recovered_messages: Vec<Message>,
    pub open_resume_picker: bool,
    /// Take-once CLI prompt. After the first frame, the TUI submits it as the
    /// first turn once the composer is free.
    pub startup_prompt: Option<String>,
}

pub struct ApplicationServices {
    pub(crate) config_repository: ConfigRepository,
    /// Theme id from the already-loaded startup config, so the TUI does not
    /// re-read and re-parse the config file for one field.
    pub(crate) theme: String,
    /// Set when this launch should open the first-run setup screen, and at
    /// which step. `None` for a returning session.
    pub(crate) first_run: Option<first_run::SetupEntry>,
    pub auth_unavailable: Option<String>,
    pub update_notice: Option<String>,
    pub pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pub pending_custom_models: Option<tokio::task::JoinHandle<()>>,
    /// Bat grammar dump loading off the UI thread. First paint stays plain if
    /// this is still running; completion rebuilds history so roles appear.
    pub pending_syntax_warmup: Option<tokio::task::JoinHandle<()>>,
    pub pending_prompt_history: Option<crate::prompt_history::PromptHistoryLoadHandle>,
    pub diagnostics: crate::diagnostics::RuntimeDiagnostics,
    pub herdr: HerdrReporter,
}
pub(crate) use attachment::{
    run as run_attachment, translate_run_event, AttachmentDisplaySettings,
};
pub(crate) use exit_receipt::{print_exit_receipt, ExitReceipt};

pub(crate) async fn run(
    agent: &mut InteractiveRuntime,
    info: TuiBootstrap,
) -> anyhow::Result<Option<ExitReceipt>> {
    let mut terminal = ratatui::init();
    Theme::initialize_from_terminal();
    Theme::apply_committed(&info.services.theme);
    let herdr = info.services.herdr.clone();
    let pending_herdr_graphics = {
        let herdr = herdr.clone();
        tokio::spawn(
            async move { herdr.graphics_capability().await }
                .instrument(tracing::info_span!("startup.herdr_graphics")),
        )
    };
    let initial_state = if info.services.auth_unavailable.is_some() {
        HerdrState::Blocked
    } else {
        HerdrState::Idle
    };
    {
        let herdr = herdr.clone();
        let message = info.services.auth_unavailable.clone();
        let session_id = info.session.session_id.clone();
        tokio::spawn(async move {
            herdr
                .report_state(initial_state, message.as_deref(), session_id.as_deref())
                .await;
        });
    }
    let result = {
        let injected = smoke_injection::after_terminal_init();

        match injected {
            Ok(()) => {
                let mut app = App::new(
                    info,
                    crate::herdr::HerdrGraphicsCapability::NotHerdr,
                    agent.mcp_report().clone(),
                    agent.mcp_catalog().clone(),
                    agent.plugins_report().clone(),
                );
                app.pending_herdr_graphics = Some(pending_herdr_graphics);
                app.terminal_session = Some(TerminalSession::acquire());
                if let Some(manager) = agent.subagents() {
                    app.subagent_inbox.bind(manager);
                    let pool = manager.concurrency();
                    app.info
                        .services
                        .diagnostics
                        .update_agent_concurrency(pool.total_limit());
                    app.agent_concurrency = Some(pool);
                }
                let result = app.run(&mut terminal, agent).await;
                if let Some(manager) = agent.subagents() {
                    manager.unbind_host_input();
                    manager.unbind_notices();
                }
                result
            }
            Err(error) => Err(error),
        }
    };
    herdr.release().await;
    ratatui::restore();
    result
}

struct App {
    info: TuiBootstrap,
    terminal_session: Option<TerminalSession>,
    statusline: StatusLine,
    subagent_panel: SubagentPanel,
    process_panel: ProcessPanel,
    subagent_inbox: subagent_inbox::SubagentInbox,
    /// Live delegated-agent cap, shared with the executor so `/config` can resize it.
    agent_concurrency: Option<crate::app::agent_concurrency::AgentConcurrency>,
    pending_subagent_questionnaire: Option<PendingSubagentQuestionnaire>,
    input_ui: InputUi,
    /// Palette match caches shared by keystroke and render paths.
    palette_caches: palette::PaletteCaches,
    /// Tiny disappearing feedback toast. Write only through [`App::set_status`]
    /// / [`App::notify_status`].
    status_overlay: Option<status_overlay::StatusOverlay>,
    /// Last status text for callers that inspect mode feedback.
    last_status: String,
    /// Who owns `last_status`, so a poll can retire its own message.
    status_source: StatusSource,
    should_quit: bool,
    ctrl_c_streak: u8,
    /// Set when a provider turn finished while history was scrolled away from
    /// the bottom, so the jump chip can flag the response until the user
    /// returns to bottom or the next turn starts.
    turn_finished_attention: bool,
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
    /// Who owns the full terminal. Setup and attach replace session chrome.
    exclusive: exclusive_screen::ExclusiveOccupant,
    pending_usage_limits: Vec<limits_command::PendingUsageFetch>,
    pending_doctor_probes: Vec<doctor_overlay::PendingDoctorProbe>,
    usage_limits_live: std::collections::BTreeMap<
        crate::usage_limits::UsageProviderKind,
        limits_command::LiveUsage,
    >,
    pending_changelog: Option<tokio::task::JoinHandle<changelog_command::ChangelogFetchResult>>,
    /// Built on first `/limits` use; constructing a client loads TLS roots,
    /// which startup should not pay for a feature that may never run.
    usage_limits_client: std::sync::OnceLock<reqwest::Client>,
    usage: UsageUi,
    model_metadata: Option<ModelMetadata>,
    pending_model_metadata: Option<tokio::task::JoinHandle<Option<ModelMetadata>>>,
    pending_model_metadata_reasoning: Option<(ReasoningLevel, ReasoningRequestSource)>,
    pending_update_notice: Option<tokio::task::JoinHandle<Option<String>>>,
    pending_custom_models: Option<tokio::task::JoinHandle<()>>,
    pending_cursor_models: Option<crate::cursor_runtime::models::RefreshHandle>,
    pending_syntax_warmup: Option<tokio::task::JoinHandle<()>>,
    prompt_history: prompt_history::PromptHistory,
    pending_herdr_graphics: Option<tokio::task::JoinHandle<HerdrGraphicsCapability>>,
    pending_github_pr: Option<tokio::task::JoinHandle<github_pr::GithubPrLookup>>,
    /// Turns held until MCP connect settles.
    held_turns: VecDeque<idle_input::HeldTurn>,
    compact_follow_up: compact_work::CompactFollowUp,
    /// When set, start the next queued follow-up once the composer is free.
    /// The bool is whether that start may auto-compact.
    start_follow_ups: Option<bool>,
    pending_model_selection: Option<InteractiveModelSelection>,
    /// Explicit all/pinned choice from the scope-toggle key. `None` means
    /// prefer pinned, falling back to all when no pin has auth.
    model_picker_scope_override: Option<model_picker::ModelPickerScope>,
    internal_agent_model_target: Option<agent_picker::InternalAgentModelTarget>,
    /// Set when the user dismisses the startup Auto classifier picker. The next
    /// idle reconcile demotes Auto → Supervised so cancel stays sync and never
    /// needs an optional runtime handle on shared picker Esc paths.
    pending_auto_classifier_demote: bool,
    agent_editor_session: Option<agent_editor::AgentEditSession>,
    sessions_hub_state: sessions_hub::SessionsHubState,
    pending_session_title: Option<PendingSessionTitle>,
    /// Set by `/title` so auto-title generation cannot overwrite a manual name.
    session_title_locked: bool,
    clipboard: Box<dyn Clipboard + Send>,
    media_attach_tasks: Vec<media_attach::MediaAttachTask>,
    /// Shared composer attachment layout for the current frame/width.
    composer_attachment_layout_cache: Option<composer_attachments::ComposerAttachmentLayoutCache>,

    /// `/attach` starts on running runs; Ctrl-R includes finished transcripts.
    attach_run_filter: attach_picker::WorkspaceRunFilter,
    /// Disk listing captured when `/attach` opens so panel ticks only rematch live rows.
    attach_disk_candidates: Vec<attach_picker::AttachCandidate>,
    /// Run ids seen on the live panel while the picker is open.
    attach_seen_live: std::collections::HashSet<String>,
    last_mouse_position: Option<(u16, u16)>,
    /// Screen-space drag selection for text outside the history area.
    screen_selection: Option<TextSelection>,
    /// MCP inventory for `/mcp` and `/doctor` (session snapshot from tool assembly).
    mcp_report: crate::tools::mcp::McpSessionReport,
    /// Prompts and resources connected MCP servers offer, for palette matching.
    mcp_catalog: crate::tools::mcp::McpCatalog,
    /// Fetched argument suggestions for the MCP prompt being typed, so palette
    /// matching reads a local cache instead of awaiting a server.
    mcp_argument_completions: mcp_argument_completion::McpArgumentCompletions,
    /// Agent Plugins load report captured at session start for `/doctor`.
    plugins_report: crate::plugins::PluginLoadReport,
    /// In-memory `/side` aside. Survives overlay close until `/new` or resume.
    side_chat: Option<side_chat::SideChat>,
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
