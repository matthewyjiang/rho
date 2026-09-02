//! Built-in Rho TUI PTY scenarios.

mod activity_anchor;
mod advisor;
mod assert_helpers;
mod attach_picker;
mod attach_view;
mod background_agents;
mod changelog;
mod command_palette;
mod config;
mod conversation_tree;
mod doctor;
mod document_attachment;
mod edit_diff;
mod file_palette;
mod first_run;
mod goal;
mod hooks;
mod limits;
mod login;
mod markdown_stream;
mod mcp;
mod mermaid;
mod paste;
mod pickers;
mod process_rail;
mod resume_delete;
mod resume_scrollback;
mod runtime_info;
mod sessions_hub;
mod side_chat;
mod startup;
mod statusline;
mod steering;
mod subagent_rail;
mod supervised_approval;
mod text_selection;
mod thermos;
mod tool_card_hover;
mod type_during_stream;
mod workflow;
mod workspace_rewind;

use activity_anchor::{SPINNER_ACTIVITY_ANCHOR_SCENARIO, SPINNER_ACTIVITY_JUMP_RAIL_SCENARIO};
use advisor::{
    setup_advisor_ready, setup_advisor_without_model, ADVISOR_COMMAND_STEPS,
    ADVISOR_MISSING_MODEL_STEPS, ADVISOR_REVIEW_STEPS, XAI_KEY_ENV,
};
use attach_picker::{
    ATTACH_CLI_EMPTY_SCENARIO, ATTACH_PICKER_EMPTY_SCENARIO, ATTACH_PICKER_SCENARIO,
};
use attach_view::{
    ATTACH_VIEW_CYCLE_SCENARIO, ATTACH_VIEW_FROM_COMMAND_SCENARIO,
    ATTACH_VIEW_PARENT_APPROVAL_SCENARIO, ATTACH_VIEW_QUIT_RESTORES_SCENARIO,
};
use background_agents::{
    BACKGROUND_AGENT_AUTO_DELIVERY_STEPS, BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
};
use changelog::CHANGELOG_STEPS;
use command_palette::{
    CREATE_AGENT_COMMAND_SCENARIO, CREATE_AGENT_MISSING_TOOLS_SCENARIO, HELP_OVERLAY_SCENARIO,
    SLASH_COMMAND_PALETTE_SCENARIO, TAB_COMPLETE_ENTER_BARE_COMMAND_SCENARIO,
};
use config::{
    setup_auto_without_classifier, AUTO_PERMISSION_MODE_CONFIG_STEPS,
    AUTO_PERMISSION_MODE_STARTUP_STEPS, OPEN_CONFIG_PICKER_STEPS,
};
use conversation_tree::CONVERSATION_TREE_STEPS;
use doctor::DOCTOR_OVERLAY_SCENARIO;
use document_attachment::DOCUMENT_ATTACHMENT_SCENARIO;
use edit_diff::EDIT_DIFF_SCENARIO;
use file_palette::FILE_PATH_AUTOCOMPLETE_SCENARIO;
use first_run::{
    setup_prompt_template, FIRST_RUN_ENV, FIRST_RUN_SETUP_STEPS, FIRST_RUN_SIGNIN_ENV,
    FIRST_RUN_SKIP_STEPS, SIGNED_OUT_SETUP_STEPS,
};
use goal::{
    GOAL_BLOCKED_AND_RESUMED_STEPS, GOAL_QUESTIONNAIRE_STEPS,
    GOAL_WAITS_FOR_SUBAGENTS_DURING_RETRY_STEPS, GOAL_WAITS_FOR_SUBAGENTS_STEPS,
};
use hooks::HOOKS_CONTRACT_SCENARIO;
use limits::LIMITS_OVERLAY_SCENARIO;
use login::{LOGIN_CUSTOM_PROVIDER_STEPS, LOGIN_OLLAMA_STEPS, LOGIN_PROVIDER_GROUPS_STEPS};
use markdown_stream::{MARKDOWN_HEADINGS_SCENARIO, STREAMING_MARKDOWN_STABILITY_SCENARIO};
use mcp::{
    MCP_CONNECTING_SCENARIO, MCP_CONNECT_RELEASE_SCENARIO, MCP_HOLD_TAKE_BACK_SCENARIO,
    MCP_INVENTORY_SCENARIO,
};
use mermaid::MERMAID_FLOWCHART_RESIZE_STEPS;
use paste::PASTE_MULTILINE_SCENARIO;
use pickers::{
    setup_edit_user_agent, setup_pinned_models, CYCLE_AND_PINNED_MODEL_PICKER_STEPS,
    EDIT_USER_AGENT_STEPS, OPENAI_AND_XAI_KEY_ENV, OPENAI_KEY_ENV, OPEN_AGENTS_PICKER_STEPS,
    OPEN_MODEL_PICKER_STEPS, OPEN_WORKFLOW_HUB_EMPTY_STEPS,
};
use process_rail::{
    PENDING_INPUT_BELOW_ACTIVITY_SCENARIO, PROCESS_RAIL_PEEK_SCENARIO, PROCESS_RAIL_SCENARIO,
};
use resume_delete::RESUME_PICKER_DELETE_STEPS;
use resume_scrollback::RESUME_SCROLLBACK_ID;
use runtime_info::RUNTIME_INFO_STEPS;
use sessions_hub::{setup_sessions_hub, SESSIONS_HUB_STEPS};
use side_chat::{
    SIDE_BTW_SCENARIO, SIDE_DURING_TURN_SCENARIO, SIDE_OVERLAY_SCENARIO, SIDE_TOGGLE_SCENARIO,
};
use startup::{
    STARTUP_FIRST_FRAME_SCENARIO, STARTUP_PROMPT_STREAM_EXIT_SCENARIO, STARTUP_STREAM_EXIT_SCENARIO,
};
use statusline::STATUSLINE_HIERARCHY_STEPS;
use std::time::Duration;
use steering::{
    QUEUE_FOLLOW_UP_DURING_TURN_SCENARIO, RETRACT_STEERING_DURING_TOOL_SCENARIO,
    STEER_APPEARS_IN_TRANSCRIPT_SCENARIO,
};
use subagent_rail::SUBAGENT_RAIL_MOUSE_SCENARIO;
use supervised_approval::SUPERVISED_APPROVAL_STEPS;
use text_selection::{SCREEN_TEXT_SELECTION_STEPS, TEXT_SELECTION_DRAG_STEPS};
use thermos::THERMOS_MISSING_WORKFLOW_SCENARIO;
use tool_card_hover::TOOL_CARD_HOVER_STEPS;
use type_during_stream::TYPE_DURING_STREAM_STEPS;
use workflow::{WORKFLOW_CANCEL_RESUME_ID, WORKFLOW_RUN_ID};
use workspace_rewind::WORKSPACE_REWIND_SCENARIO;

use anyhow::Result;

use crate::{
    harness::WaitTimeout,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner, Step},
};

const DEFAULT_SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

pub(super) const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
pub(super) const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
pub(super) const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

const TYPE_DURING_COMPACT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture compact until cancel"),
    Step::WaitText {
        text: "fixture response: fixture compact until cancel",
        timeout: STREAM,
    },
    Step::Phase("compact"),
    Step::SubmitText("/compact"),
    Step::WaitText {
        text: "compacting context",
        timeout: STREAM,
    },
    Step::Phase("type_draft"),
    Step::TypeText("draft during compact"),
    Step::WaitText {
        text: "draft during compact",
        timeout: WaitTimeout::secs(2, "composer input during compact"),
    },
    Step::Phase("cancel_compact"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "context compaction cancelled",
        timeout: WaitTimeout::secs(2, "esc cancels compact"),
    },
    Step::WaitText {
        text: "draft during compact",
        timeout: WaitTimeout::secs(2, "draft survives compact cancel"),
    },
    Step::CtrlCExit,
];

const SUBMIT_DURING_COMPACT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("seed_history"),
    Step::SubmitText("fixture compact until release"),
    Step::WaitText {
        text: "fixture response: fixture compact until release",
        timeout: STREAM,
    },
    Step::Phase("compact"),
    Step::SubmitText("/compact"),
    Step::WaitText {
        text: "compacting context",
        timeout: STREAM,
    },
    Step::Phase("submit_follow_up"),
    Step::SubmitText("after compact please"),
    Step::WaitText {
        text: "1 follow-up",
        timeout: WaitTimeout::secs(2, "queued follow-up during compact"),
    },
    Step::Phase("release_compact"),
    Step::Custom(release_compact_fixture),
    Step::Phase("drain_after_failed_compact"),
    Step::WaitText {
        text: "fixture response: after compact please",
        timeout: STREAM,
    },
    Step::CtrlCExit,
];

const CANCEL_AND_RESUBMIT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_delay"),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::Phase("cancel"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::Phase("resubmit"),
    Step::SubmitText("hello after cancel"),
    Step::WaitText {
        text: "fixture response: hello after cancel",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

const INLINE_SHELL_DURING_TURN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("!!printf idle-stream-%s start; sleep 2; printf idle-stream-%s end"),
    Step::WaitText {
        text: "idle-stream-start",
        timeout: STREAM,
    },
    Step::Custom(assert_idle_shell_still_streaming),
    Step::WaitText {
        text: "idle-stream-end",
        timeout: STREAM,
    },
    Step::SubmitText("!!printf cancel-%s started; sleep 1; printf cancel-%s escaped-output"),
    Step::WaitText {
        text: "cancel-started",
        timeout: STREAM,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "cancelled",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(1_200),
        timeout: STREAM,
    },
    Step::Custom(assert_inline_shell_cancelled),
    Step::SubmitText("fixture delay"),
    Step::WaitText {
        text: "partial assistant before cancellation",
        timeout: STREAM,
    },
    Step::SubmitText("!!printf streamed-%s start; sleep 1; printf streamed-%s end"),
    Step::WaitText {
        text: "streamed-start",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "streamed-end",
        timeout: STREAM,
    },
    Step::SubmitText("!printf context-%s during-turn"),
    Step::WaitText {
        text: "context-during-turn",
        timeout: STREAM,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const RESIZE_DURING_STREAM_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("stream"),
    Step::SubmitText("fixture stream"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::Phase("resize"),
    Step::Resize { rows: 20, cols: 70 },
    Step::Resize {
        rows: 32,
        cols: 120,
    },
    Step::Resize {
        rows: 28,
        cols: 100,
    },
    Step::WaitText {
        text: "part two",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const SCROLL_DURING_STREAM_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("continuous_stream"),
    Step::SubmitText("fixture scroll checkpoint"),
    Step::WaitText {
        text: "scroll checkpoint event 100",
        timeout: STREAM,
    },
    Step::Phase("scroll_up"),
    Step::Key(Key::PageUp),
    Step::Key(Key::PageUp),
    Step::WaitText {
        text: "scroll checkpoint event 050",
        timeout: WaitTimeout::millis(500, "scroll during stream"),
    },
    Step::Phase("return_bottom"),
    Step::Key(Key::CtrlEnd),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

const TERMINAL_RESTORATION_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    // Entering the TUI enables alternate screen / mouse / paste modes.
    Step::AssertRawContains("\u{1b}[?1049h"),
    Step::ExitCommand,
    Step::Phase("assert_restore"),
    Step::Custom(assert_terminal_restored),
];

const QUESTIONNAIRE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture questionnaire"),
    Step::WaitText {
        text: "Choose one color",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "A warm primary color",
        timeout: STREAM,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "questionnaire response observed exactly 1 time",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

const PROGRESS_TOOL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture progress tool"),
    Step::WaitText {
        text: "deterministic fixture tool result",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "progress tool lifecycle complete",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

const CONCURRENT_PROGRESS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture concurrent progress"),
    Step::WaitText {
        text: "slow fixture progress one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "fast fixture result",
        timeout: STREAM,
    },
    Step::AssertText("slow fixture progress one"),
    Step::WaitText {
        text: "slow fixture result",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "concurrent progress complete in model order",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

/// All registered scenarios.
const ALL_SCENARIOS: &[Scenario] = &[
    STARTUP_FIRST_FRAME_SCENARIO,
    STARTUP_STREAM_EXIT_SCENARIO,
    STARTUP_PROMPT_STREAM_EXIT_SCENARIO,
    Scenario::new(
        "cancel_and_resubmit",
        "Cancel a long fixture stream and submit another prompt",
        DEFAULT_SIZE,
        CANCEL_AND_RESUBMIT_STEPS,
        true,
    ),
    Scenario::new(
        "inline_shell_during_turn",
        "Run local and context shell commands during an active turn",
        DEFAULT_SIZE,
        INLINE_SHELL_DURING_TURN_STEPS,
        false,
    ),
    Scenario::new(
        "type_during_stream",
        "Keep composer input responsive; overlay Esc must not abort, empty Esc must",
        DEFAULT_SIZE,
        TYPE_DURING_STREAM_STEPS,
        true,
    ),
    Scenario::new(
        "type_during_compact",
        "Keep composer input responsive while /compact is running",
        DEFAULT_SIZE,
        TYPE_DURING_COMPACT_STEPS,
        false,
    ),
    Scenario::new(
        "submit_during_compact",
        "A prompt submitted during /compact starts after compaction ends",
        DEFAULT_SIZE,
        SUBMIT_DURING_COMPACT_STEPS,
        false,
    ),
    Scenario::new(
        "resize_during_stream",
        "Resize repeatedly while a fixture stream is active",
        DEFAULT_SIZE,
        RESIZE_DURING_STREAM_STEPS,
        true,
    ),
    Scenario::new(
        "scroll_during_stream",
        "Scroll during bulk output and return to bottom",
        DEFAULT_SIZE,
        SCROLL_DURING_STREAM_STEPS,
        true,
    ),
    Scenario::new(
        "terminal_restoration",
        "Verify alternate-screen enter/leave around a clean exit",
        DEFAULT_SIZE,
        TERMINAL_RESTORATION_STEPS,
        true,
    ),
    Scenario::new(
        WORKFLOW_RUN_ID,
        "Confirm and observe a workflow in its separate terminal mode",
        DEFAULT_SIZE,
        &[],
        true,
    ),
    Scenario::new(
        WORKFLOW_CANCEL_RESUME_ID,
        "Cancel, save, and resume a workflow without rerunning completed nodes",
        DEFAULT_SIZE,
        &[],
        true,
    ),
    PASTE_MULTILINE_SCENARIO,
    DOCUMENT_ATTACHMENT_SCENARIO,
    Scenario::new(
        "questionnaire",
        "Exercise questionnaire keyboard selection and submission",
        DEFAULT_SIZE,
        QUESTIONNAIRE_STEPS,
        false,
    ),
    Scenario::new(
        "supervised_approval",
        "Inspect and cancel a bounded supervised process approval",
        PtySize {
            rows: 14,
            cols: 100,
        },
        SUPERVISED_APPROVAL_STEPS,
        true,
    ),
    Scenario::new(
        "auto_permission_mode_config",
        "Gate Auto behind a classifier model picker, cancel safely, then enable it",
        PtySize {
            rows: 14,
            cols: 100,
        },
        AUTO_PERMISSION_MODE_CONFIG_STEPS,
        /*smoke*/ false,
    )
    .with_env(OPENAI_KEY_ENV),
    Scenario::new(
        "auto_permission_mode_startup",
        "Start in Auto without a classifier and force a model pick before tools run",
        PtySize {
            rows: 14,
            cols: 100,
        },
        AUTO_PERMISSION_MODE_STARTUP_STEPS,
        /*smoke*/ false,
    )
    .with_setup(setup_auto_without_classifier)
    .with_env(OPENAI_KEY_ENV),
    // Custom multi-process scenario: see config::run_auto_recovered_handoff.
    Scenario::new(
        config::AUTO_PERMISSION_MODE_RECOVERED_HANDOFF_ID,
        "Resume a session under Auto without a classifier and force the model pick",
        PtySize {
            rows: 16,
            cols: 100,
        },
        &[],
        /*smoke*/ false,
    ),
    Scenario::new(
        "progress_tool",
        "Run the fixture progress tool to completion",
        DEFAULT_SIZE,
        PROGRESS_TOOL_STEPS,
        false,
    ),
    EDIT_DIFF_SCENARIO,
    Scenario::new(
        "concurrent_progress",
        "Keep concurrent progress visible through out-of-order completion",
        DEFAULT_SIZE,
        CONCURRENT_PROGRESS_STEPS,
        false,
    ),
    STEER_APPEARS_IN_TRANSCRIPT_SCENARIO,
    RETRACT_STEERING_DURING_TOOL_SCENARIO,
    QUEUE_FOLLOW_UP_DURING_TURN_SCENARIO,
    MARKDOWN_HEADINGS_SCENARIO,
    STREAMING_MARKDOWN_STABILITY_SCENARIO,
    SPINNER_ACTIVITY_ANCHOR_SCENARIO,
    SPINNER_ACTIVITY_JUMP_RAIL_SCENARIO,
    HELP_OVERLAY_SCENARIO,
    LIMITS_OVERLAY_SCENARIO,
    DOCTOR_OVERLAY_SCENARIO,
    SIDE_OVERLAY_SCENARIO,
    SIDE_TOGGLE_SCENARIO,
    SIDE_BTW_SCENARIO,
    SIDE_DURING_TURN_SCENARIO,
    SLASH_COMMAND_PALETTE_SCENARIO,
    CREATE_AGENT_COMMAND_SCENARIO,
    CREATE_AGENT_MISSING_TOOLS_SCENARIO,
    TAB_COMPLETE_ENTER_BARE_COMMAND_SCENARIO,
    FILE_PATH_AUTOCOMPLETE_SCENARIO,
    Scenario::new(
        "mermaid_flowchart_resize",
        "Render a long-labelled flowchart, then explain the fallback in a narrow pane",
        DEFAULT_SIZE,
        MERMAID_FLOWCHART_RESIZE_STEPS,
        false,
    ),
    Scenario::new(
        "runtime_info",
        "Show grouped runtime details and keep them readable after a narrow resize",
        DEFAULT_SIZE,
        RUNTIME_INFO_STEPS,
        false,
    ),
    Scenario::new(
        "statusline_hierarchy",
        "Keep ranked statusline identity fields as the terminal narrows",
        DEFAULT_SIZE,
        STATUSLINE_HIERARCHY_STEPS,
        false,
    ),
    Scenario::new(
        "changelog",
        "Show bundled release notes for the installed version in chat",
        DEFAULT_SIZE,
        CHANGELOG_STEPS,
        false,
    ),
    Scenario::new(
        "conversation_tree",
        "Restore an earlier turn and continue on a new branch",
        DEFAULT_SIZE,
        CONVERSATION_TREE_STEPS,
        false,
    ),
    WORKSPACE_REWIND_SCENARIO,
    HOOKS_CONTRACT_SCENARIO,
    MCP_INVENTORY_SCENARIO,
    MCP_CONNECTING_SCENARIO,
    MCP_CONNECT_RELEASE_SCENARIO,
    MCP_HOLD_TAKE_BACK_SCENARIO,
    Scenario::new(
        "resume_picker_delete",
        "Delete a saved session from the resume picker with confirm/cancel",
        DEFAULT_SIZE,
        RESUME_PICKER_DELETE_STEPS,
        false,
    ),
    Scenario::new(
        RESUME_SCROLLBACK_ID,
        "Resume a long session and page up to earlier transcript rows",
        DEFAULT_SIZE,
        &[],
        /*smoke*/ false,
    ),
    Scenario::new(
        "sessions_hub",
        "Inspect a foreign session safely, then browse, resume, and delete locally",
        DEFAULT_SIZE,
        SESSIONS_HUB_STEPS,
        false,
    )
    .with_setup(setup_sessions_hub),
    Scenario::new(
        "open_model_picker",
        "Open and dismiss the model picker",
        DEFAULT_SIZE,
        OPEN_MODEL_PICKER_STEPS,
        false,
    )
    .with_env(OPENAI_KEY_ENV),
    Scenario::new(
        "cycle_and_pinned_model_picker",
        "Cycle pinned models from the composer and toggle the /model pinned view",
        DEFAULT_SIZE,
        CYCLE_AND_PINNED_MODEL_PICKER_STEPS,
        false,
    )
    .with_setup(setup_pinned_models)
    .with_env(OPENAI_AND_XAI_KEY_ENV),
    Scenario::new(
        "open_workflow_hub_empty",
        "Open the workflows hub when the workspace has no workflows yet",
        DEFAULT_SIZE,
        OPEN_WORKFLOW_HUB_EMPTY_STEPS,
        false,
    ),
    THERMOS_MISSING_WORKFLOW_SCENARIO,
    Scenario::new(
        "open_config_picker",
        "Open model and provider settings and browse model refresh options",
        DEFAULT_SIZE,
        OPEN_CONFIG_PICKER_STEPS,
        false,
    ),
    Scenario::new(
        "open_agents_picker",
        "Browse agent metadata in a navigable popup and scroll hidden detail into view",
        DEFAULT_SIZE,
        OPEN_AGENTS_PICKER_STEPS,
        false,
    )
    .with_env(OPENAI_KEY_ENV),
    Scenario {
        id: "edit_user_agent",
        description: "Edit and save a user-defined agent through the agents picker",
        size: DEFAULT_SIZE,
        setup: Some(setup_edit_user_agent),
        env: &[],
        args: &[],
        steps: EDIT_USER_AGENT_STEPS,
        smoke: false,
    },
    Scenario::new(
        "first_run_setup",
        "Walk a first launch through the full-screen sign-in and model steps",
        DEFAULT_SIZE,
        FIRST_RUN_SETUP_STEPS,
        /*smoke*/ false,
    )
    .with_env(FIRST_RUN_SIGNIN_ENV),
    Scenario::new(
        "first_run_setup_skipped",
        "Leave the first-launch setup screen with Esc and land in a session",
        DEFAULT_SIZE,
        FIRST_RUN_SKIP_STEPS,
        /*smoke*/ false,
    )
    .with_env(FIRST_RUN_ENV),
    Scenario::new(
        "signed_out_setup_state",
        "Show the signed-out header and statusline, and route a prompt to login",
        DEFAULT_SIZE,
        SIGNED_OUT_SETUP_STEPS,
        /*smoke*/ false,
    )
    .with_setup(setup_prompt_template),
    Scenario::new(
        "login_provider_groups",
        "Group login providers and open readable authentication methods",
        DEFAULT_SIZE,
        LOGIN_PROVIDER_GROUPS_STEPS,
        false,
    ),
    Scenario::new(
        "login_custom_provider",
        "Create a custom OpenAI-compatible host from /login without an API key",
        DEFAULT_SIZE,
        LOGIN_CUSTOM_PROVIDER_STEPS,
        false,
    ),
    Scenario::new(
        "login_ollama",
        "Configure the Ollama endpoint from /login without an API key",
        DEFAULT_SIZE,
        LOGIN_OLLAMA_STEPS,
        false,
    ),
    Scenario::new(
        "goal_blocked_and_resumed",
        "Pause a goal for user action, inspect it, then resume it",
        DEFAULT_SIZE,
        GOAL_BLOCKED_AND_RESUMED_STEPS,
        false,
    ),
    Scenario::new(
        "goal_waits_for_subagents",
        "Wait for delegated runs before prompting an active goal to continue",
        DEFAULT_SIZE,
        GOAL_WAITS_FOR_SUBAGENTS_STEPS,
        false,
    ),
    Scenario::new(
        "goal_questionnaire",
        "Answer a background child questionnaire while an active goal waits",
        DEFAULT_SIZE,
        GOAL_QUESTIONNAIRE_STEPS,
        false,
    ),
    Scenario::new(
        "goal_waits_for_subagents_during_retry",
        "Wait for delegated runs before retrying a failed goal turn",
        DEFAULT_SIZE,
        GOAL_WAITS_FOR_SUBAGENTS_DURING_RETRY_STEPS,
        false,
    ),
    Scenario::new(
        "background_agent_auto_delivery",
        "Spawn a background agent, end the turn, and receive its completion automatically",
        DEFAULT_SIZE,
        BACKGROUND_AGENT_AUTO_DELIVERY_STEPS,
        false,
    ),
    SUBAGENT_RAIL_MOUSE_SCENARIO,
    ATTACH_PICKER_SCENARIO,
    ATTACH_PICKER_EMPTY_SCENARIO,
    ATTACH_CLI_EMPTY_SCENARIO,
    ATTACH_VIEW_FROM_COMMAND_SCENARIO,
    ATTACH_VIEW_CYCLE_SCENARIO,
    ATTACH_VIEW_PARENT_APPROVAL_SCENARIO,
    ATTACH_VIEW_QUIT_RESTORES_SCENARIO,
    PROCESS_RAIL_SCENARIO,
    PROCESS_RAIL_PEEK_SCENARIO,
    PENDING_INPUT_BELOW_ACTIVITY_SCENARIO,
    Scenario::new(
        "tool_card_hover",
        "Lift tool-card text on hover and expand the card on click",
        DEFAULT_SIZE,
        TOOL_CARD_HOVER_STEPS,
        false,
    ),
    Scenario::new(
        "text_selection_drag",
        "Update the drag selection highlight before the mouse button is released",
        DEFAULT_SIZE,
        TEXT_SELECTION_DRAG_STEPS,
        false,
    ),
    Scenario::new(
        "screen_text_selection",
        "Click and drag in the composer to place the caret and replace selected text",
        DEFAULT_SIZE,
        SCREEN_TEXT_SELECTION_STEPS,
        false,
    ),
    Scenario::new(
        "advisor_command",
        "Choose an advisor model, turn the mode off from config, and turn it on again",
        DEFAULT_SIZE,
        ADVISOR_COMMAND_STEPS,
        /*smoke*/ false,
    )
    .with_env(XAI_KEY_ENV),
    Scenario {
        id: "advisor_missing_model",
        description: "Warn about advisor mode saved without a model and route to a model picker",
        size: DEFAULT_SIZE,
        setup: Some(setup_advisor_without_model),
        env: XAI_KEY_ENV,
        args: &[],
        steps: ADVISOR_MISSING_MODEL_STEPS,
        smoke: false,
    },
    Scenario {
        id: "advisor_review",
        description: "Consult the advisor during a turn, then survive an advisor failure",
        size: DEFAULT_SIZE,
        setup: Some(setup_advisor_ready),
        env: XAI_KEY_ENV,
        args: &[],
        steps: ADVISOR_REVIEW_STEPS,
        smoke: false,
    },
    Scenario::new(
        "background_agent_questionnaire",
        "Answer a questionnaire raised by a background agent and deliver its completion",
        DEFAULT_SIZE,
        BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
        false,
    ),
];

pub fn all_scenarios() -> &'static [Scenario] {
    ALL_SCENARIOS
}

pub fn smoke_scenario_ids() -> Vec<&'static str> {
    all_scenarios()
        .iter()
        .filter(|scenario| scenario.smoke)
        .map(|scenario| scenario.id)
        .collect()
}

/// Release the hanging compact fixture after the follow-up is queued.
///
/// Must match `RELEASE_MARKER` in `crates/rho-providers/src/providers/tui_fixture/compact.rs`.
fn release_compact_fixture(harness: &mut crate::harness::PtyHarness) -> anyhow::Result<()> {
    let cwd = harness
        .working_directory()
        .ok_or_else(|| anyhow::anyhow!("pty harness has no working directory"))?;
    std::fs::write(cwd.join(".rho-fixture-release-compact"), b"")
        .map_err(|error| anyhow::anyhow!("write compact release marker: {error}"))
}

pub fn run_named(runner: &ScenarioRunner, name: &str) -> Result<ScenarioOutcome> {
    let scenario = all_scenarios()
        .iter()
        .find(|scenario| scenario.id == name)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario '{name}'"))?;
    if workflow::is_workflow_scenario(name) {
        return workflow::run(runner, name);
    }
    if config::is_auto_recovered_handoff_scenario(name) {
        return config::run_auto_recovered_handoff(runner);
    }
    if resume_scrollback::is_resume_scrollback_scenario(name) {
        return resume_scrollback::run_resume_scrollback(runner);
    }
    runner.run(scenario)
}

use assert_helpers::{
    assert_idle_shell_still_streaming, assert_inline_shell_cancelled, assert_terminal_restored,
};
