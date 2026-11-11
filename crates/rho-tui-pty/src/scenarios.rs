//! Built-in Rho TUI PTY scenarios.

mod activity_anchor;
mod advisor;
mod agent_messages;
mod agent_prompt;
mod assert_helpers;
mod attach_picker;
mod attach_view;
mod background_agents;
mod basic_steps;
mod boundary_notifications;
mod calibrated_context;
mod changelog;
mod code_wrap;
mod codemode_stream;
mod command_palette;
mod compact;
mod composer_unicode;
mod composer_vim;
mod config;
mod config_2x_upgrade;
mod context_overlay;
mod continue_latest;
mod conversation_tree;
mod copy_output;
// The rho daemon is unix-only (#1665).
#[cfg(unix)]
mod daemon_approval;
#[cfg(unix)]
mod daemon_config;
#[cfg(unix)]
mod daemon_context;
#[cfg(unix)]
mod daemon_detach;
#[cfg(unix)]
mod daemon_doctor;
#[cfg(unix)]
mod daemon_goal;
#[cfg(unix)]
mod daemon_login;
#[cfg(unix)]
mod daemon_mcp;
#[cfg(unix)]
mod daemon_mcp_startup;
#[cfg(unix)]
mod daemon_model;
#[cfg(unix)]
mod daemon_options;
#[cfg(unix)]
mod daemon_prompt;
#[cfg(unix)]
mod daemon_reports;
#[cfg(unix)]
mod daemon_restart;
#[cfg(unix)]
mod daemon_resume;
#[cfg(unix)]
mod daemon_rewind;
#[cfg(unix)]
mod daemon_subagents;
#[cfg(unix)]
mod daemon_switch;
#[cfg(unix)]
mod daemon_tree;
// Starts a real command workflow, as `workflow_background` does.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod daemon_workflow;
mod diff_viewer;
mod dispatch;
mod doctor;
mod document_attachment;
mod edit_diff;
mod fast_discovery;
mod file_palette;
mod first_run;
mod goal;
#[cfg(unix)]
mod header_hints;
mod hooks;
mod hover_clicks;
mod init_command;
mod inline_choice_click;
mod inline_shell;
mod limits;
mod line_editor;
mod login;
mod markdown_stream;
mod mcp;
mod mermaid;
mod model_cycle_reasoning;
mod model_prompts;
mod new_session_prompt;
mod no_save;
mod palette_click;
mod panel_pointer;
mod paste;
mod permission_cycle;
mod pickers;
mod plan_exit;
mod process_rail;
mod program_status;
mod prompt_history_search;
mod questionnaire;
mod questionnaire_timeout;
#[cfg(unix)]
mod quiet_subagent;
#[cfg(unix)]
mod reasoning_output;
mod remember;
mod resume_delete;
mod resume_scrollback;
mod run_events;
mod runtime_info;
mod send_confirm;
mod sessions_hub;
mod sessions_tool;
mod shell_completion;
mod side_chat;
mod spend;
mod startup;
mod status_clicks;
mod statusline;
mod steering;
mod streaming_controls;
mod subagent_rail;
mod supervised_approval;
#[cfg(unix)]
mod terminal_ownership;
mod text_selection;
mod todo_card;
mod todo_progress;
mod tool_card_hover;
mod transcript_search;
mod turn_notifications;
mod type_during_stream;
mod workflow;
// Real command workflows require the verified-handle process adapter, not just a PTY.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod workflow_background;
mod workflow_hub_unsupported;
mod workspace_rewind;
mod write_stream;
mod zen_tool_run;

use activity_anchor::{SPINNER_ACTIVITY_ANCHOR_SCENARIO, SPINNER_ACTIVITY_JUMP_RAIL_SCENARIO};
use advisor::{ADVISOR_MISSING_MODEL_SCENARIO, ADVISOR_REVIEW_SCENARIO};
use agent_prompt::AGENT_PROMPT_SCENARIO;
use attach_picker::ATTACH_PICKER_SCENARIO;
use attach_view::{
    ATTACH_PARENT_MESSAGE_SCENARIO, ATTACH_VIEW_CYCLE_SCENARIO, ATTACH_VIEW_FROM_COMMAND_SCENARIO,
    ATTACH_VIEW_PARENT_APPROVAL_SCENARIO, ATTACH_VIEW_QUIT_RESTORES_SCENARIO,
};
use background_agents::{
    BACKGROUND_AGENT_AUTO_DELIVERY_STEPS, BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
    LATE_BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
};
use changelog::CHANGELOG_STEPS;
use command_palette::{
    CREATE_AGENT_COMMAND_SCENARIO, CREATE_AGENT_MISSING_TOOLS_SCENARIO,
    PROMPT_TEMPLATE_DURING_TURN_SCENARIO, PROMPT_TEMPLATE_HOT_RELOAD_SCENARIO,
    TAB_COMPLETE_ENTER_BARE_COMMAND_SCENARIO,
};
use compact::{SUBMIT_DURING_COMPACT_STEPS, TYPE_DURING_COMPACT_STEPS};
use config::{setup_auto_without_classifier, AUTO_PERMISSION_MODE_STARTUP_STEPS};
use conversation_tree::CONVERSATION_TREE_STEPS;
use doctor::{DOCTOR_MCP_SCENARIO, DOCTOR_OVERLAY_SCENARIO};
use document_attachment::DOCUMENT_ATTACHMENT_SCENARIO;
use edit_diff::EDIT_DIFF_SCENARIO;
use file_palette::FILE_PATH_AUTOCOMPLETE_SCENARIO;
use first_run::{
    setup_prompt_template, FIRST_RUN_DECISION_HOST_STEPS, FIRST_RUN_ENV, FIRST_RUN_SETUP_STEPS,
    FIRST_RUN_SIGNIN_ENV, FIRST_RUN_SKIP_STEPS, SIGNED_OUT_SETUP_STEPS,
};
use goal::{
    GOAL_BLOCKED_AND_RESUMED_STEPS, GOAL_QUESTIONNAIRE_STEPS,
    GOAL_WAITS_FOR_SUBAGENTS_DURING_RETRY_STEPS, GOAL_WAITS_FOR_SUBAGENTS_STEPS,
};
#[cfg(unix)]
use header_hints::HEADER_HINTS_SCENARIO;
use hooks::HOOKS_CONTRACT_SCENARIO;
use init_command::{INIT_COMMAND_SCENARIO, INIT_EXISTING_SCENARIO, INIT_PLAN_SCENARIO};
use inline_shell::INLINE_SHELL_DURING_TURN_SCENARIO;
use limits::LIMITS_OVERLAY_SCENARIO;
use login::{
    LOGIN_CUSTOM_PROVIDER_STEPS, LOGIN_OAUTH_FLOW_CHOICE_SCENARIO, LOGIN_OLLAMA_STEPS,
    LOGIN_PROVIDER_GROUPS_STEPS,
};
use markdown_stream::{MARKDOWN_HEADINGS_SCENARIO, STREAMING_MARKDOWN_STABILITY_SCENARIO};
use mcp::{
    MCP_CONNECTING_SCENARIO, MCP_CONNECT_RELEASE_SCENARIO, MCP_HOLD_TAKE_BACK_SCENARIO,
    MCP_INVENTORY_SCENARIO,
};
use mermaid::MERMAID_FLOWCHART_RESIZE_STEPS;
use no_save::NO_SAVE_SESSION_SCENARIO;
use paste::PASTE_MULTILINE_SCENARIO;
use pickers::{
    setup_edit_user_agent, setup_invalid_user_agent, setup_pinned_models,
    CYCLE_AND_PINNED_MODEL_PICKER_STEPS, DELETE_USER_AGENT_STEPS, EDIT_USER_AGENT_STEPS,
    EDIT_USER_AGENT_TOOLS_STEPS, INVALID_USER_AGENT_STEPS, OPENAI_AND_XAI_KEY_ENV, OPENAI_KEY_ENV,
    OPEN_AGENTS_PICKER_STEPS, OPEN_MODEL_PICKER_STEPS, OPEN_WORKFLOW_HUB_EMPTY_STEPS,
    PLUGIN_AGENT_MODEL_SCENARIO, VIEW_READ_ONLY_AGENT_PROMPT_STEPS,
};
use process_rail::{
    PENDING_INPUT_BELOW_ACTIVITY_SCENARIO, PROCESS_RAIL_PEEK_SCENARIO, PROCESS_RAIL_SCENARIO,
};
use program_status::PROGRAM_STATUS_SCENARIO;
use prompt_history_search::PROMPT_HISTORY_SEARCH_SCENARIO;
#[cfg(unix)]
use reasoning_output::REASONING_OUTPUT_RETROACTIVE_SCENARIO;
use resume_delete::RESUME_PICKER_DELETE_STEPS;
use resume_scrollback::RESUME_SCROLLBACK_ID;
use runtime_info::RUNTIME_INFO_STEPS;
use sessions_hub::{setup_sessions_hub, SESSIONS_HUB_STEPS};
use shell_completion::SHELL_TAB_COMPLETION_SCENARIO;
use side_chat::{
    SIDE_BTW_SCENARIO, SIDE_COMPOSER_SCENARIO, SIDE_DURING_TURN_SCENARIO, SIDE_OVERLAY_SCENARIO,
    SIDE_TOGGLE_SCENARIO,
};
use startup::{
    STARTUP_FIRST_FRAME_SCENARIO, STARTUP_PROMPT_STREAM_EXIT_SCENARIO, STARTUP_STREAM_EXIT_SCENARIO,
};
use statusline::STATUSLINE_HIERARCHY_STEPS;
use steering::{
    QUEUE_FOLLOW_UP_DURING_TURN_SCENARIO, RETRACT_STEERING_DURING_TOOL_SCENARIO,
    STEER_APPEARS_IN_TRANSCRIPT_SCENARIO, STEER_DELIVERED_MARKER_SCENARIO,
};
use streaming_controls::STREAMING_CONTROLS_SCENARIO;
use subagent_rail::SUBAGENT_RAIL_MOUSE_SCENARIO;
use supervised_approval::SUPERVISED_APPROVAL_STEPS;
use text_selection::{SCREEN_TEXT_SELECTION_STEPS, TEXT_SELECTION_DRAG_STEPS};
use tool_card_hover::TOOL_CARD_HOVER_STEPS;
use transcript_search::{TRANSCRIPT_SEARCH_HOLDS_HIT_SCENARIO, TRANSCRIPT_SEARCH_SCENARIO};
use turn_notifications::TURN_NOTIFICATIONS_SCENARIO;
use type_during_stream::TYPE_DURING_STREAM_STEPS;
use workflow::{WORKFLOW_CANCEL_RESUME_ID, WORKFLOW_RUN_ID};
use workflow_hub_unsupported::{
    setup_workflow_hub_unsupported_records, WORKFLOW_HUB_UNSUPPORTED_ID,
    WORKFLOW_HUB_UNSUPPORTED_STEPS,
};
use workspace_rewind::{WORKSPACE_REWIND_OFF_SCENARIO, WORKSPACE_REWIND_SCENARIO};
use zen_tool_run::ZEN_TOOL_RUN_SUMMARY_SCENARIO;

use crate::{
    harness::WaitTimeout,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

const DEFAULT_SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

pub(super) const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
pub(super) const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
pub(super) const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");
use basic_steps::{
    CANCEL_AND_RESUBMIT_STEPS, CONCURRENT_PROGRESS_STEPS, PROGRESS_TOOL_STEPS,
    RESIZE_DURING_STREAM_STEPS, SCROLL_DURING_STREAM_STEPS, TERMINAL_RESTORATION_STEPS,
};

/// All registered scenarios.
const ALL_SCENARIOS: &[Scenario] = &[
    INIT_COMMAND_SCENARIO,
    INIT_EXISTING_SCENARIO,
    INIT_PLAN_SCENARIO,
    STARTUP_FIRST_FRAME_SCENARIO,
    STARTUP_STREAM_EXIT_SCENARIO,
    STARTUP_PROMPT_STREAM_EXIT_SCENARIO,
    NO_SAVE_SESSION_SCENARIO,
    remember::REMEMBER_SCENARIO,
    #[cfg(unix)]
    REASONING_OUTPUT_RETROACTIVE_SCENARIO,
    ZEN_TOOL_RUN_SUMMARY_SCENARIO,
    #[cfg(unix)]
    HEADER_HINTS_SCENARIO,
    TURN_NOTIFICATIONS_SCENARIO,
    PROGRAM_STATUS_SCENARIO,
    PROMPT_HISTORY_SEARCH_SCENARIO,
    TRANSCRIPT_SEARCH_SCENARIO,
    TRANSCRIPT_SEARCH_HOLDS_HIT_SCENARIO,
    STREAMING_CONTROLS_SCENARIO,
    #[cfg(unix)]
    streaming_controls::DAEMON_STREAMING_CONTROLS_SCENARIO,
    run_events::PROVIDER_RETRY_SCENARIO,
    #[cfg(unix)]
    run_events::DAEMON_PROVIDER_RETRY_SCENARIO,
    run_events::WEB_SEARCH_ACTIVITY_SCENARIO,
    #[cfg(unix)]
    run_events::DAEMON_WEB_SEARCH_ACTIVITY_SCENARIO,
    run_events::PROVIDER_DIAGNOSTIC_SCENARIO,
    #[cfg(unix)]
    run_events::DAEMON_PROVIDER_DIAGNOSTIC_SCENARIO,
    AGENT_PROMPT_SCENARIO,
    Scenario::new(
        "cancel_and_resubmit",
        "Cancel a long fixture stream and submit another prompt",
        DEFAULT_SIZE,
        CANCEL_AND_RESUBMIT_STEPS,
        true,
    ),
    INLINE_SHELL_DURING_TURN_SCENARIO,
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
    #[cfg(any(target_os = "linux", target_os = "android"))]
    workflow_background::WORKFLOW_BACKGROUND_SCENARIO,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    workflow_background::WORKFLOW_HUB_PLANNED_RETRY_SCENARIO,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    workflow_background::DAEMON_WORKFLOW_BACKGROUND_SCENARIO,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    workflow_background::DAEMON_WORKFLOW_HUB_PLANNED_RETRY_SCENARIO,
    PASTE_MULTILINE_SCENARIO,
    composer_unicode::COMPOSER_UNICODE_SCENARIO,
    composer_vim::COMPOSER_VIM_SCENARIO,
    questionnaire_timeout::TIMEOUT,
    questionnaire_timeout::PAUSE,
    DOCUMENT_ATTACHMENT_SCENARIO,
    questionnaire::QUESTIONNAIRE_SCENARIO,
    questionnaire::QUESTIONNAIRE_CLICK_SCENARIO,
    questionnaire::APPROVAL_CLICK_SCENARIO,
    inline_choice_click::INLINE_CHOICE_CLICK_SCENARIO,
    #[cfg(unix)]
    inline_choice_click::DAEMON_DELETE_LEFT_SESSION_SCENARIO,
    palette_click::SLASH_PALETTE_CLICK_SCENARIO,
    palette_click::FILE_PALETTE_CLICK_SCENARIO,
    #[cfg(unix)]
    hover_clicks::INLINE_PICKER_CLICK_SCENARIO,
    hover_clicks::OVERLAY_PICKER_DOUBLE_CLICK_SCENARIO,
    panel_pointer::PANEL_POINTER_SCENARIO,
    panel_pointer::SIDE_POINTER_SCENARIO,
    status_clicks::STATUSLINE_MODEL_CLICK_SCENARIO,
    status_clicks::STATUSLINE_NO_REASONING_CLICK_SCENARIO,
    status_clicks::ATTACHMENT_CLICK_REMOVE_SCENARIO,
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
    config::PERMISSIONS_COMMAND_SCENARIO,
    #[cfg(unix)]
    config::SETTINGS_NEED_THE_DAEMON,
    plan_exit::APPROVE_SCENARIO,
    #[cfg(unix)]
    plan_exit::DAEMON_APPROVE_SCENARIO,
    #[cfg(unix)]
    init_command::DAEMON_INIT_PLAN_SCENARIO,
    plan_exit::GOAL_SCENARIO,
    plan_exit::IDLE_COMPLETION_SCENARIO,
    plan_exit::KEEP_SCENARIO,
    plan_exit::FAILED_SCENARIO,
    plan_exit::QUEUED_CYCLE_SCENARIO,
    #[cfg(unix)]
    plan_exit::DAEMON_GOAL_SCENARIO,
    #[cfg(unix)]
    plan_exit::DAEMON_DELEGATED_COMPLETION_SCENARIO,
    #[cfg(unix)]
    plan_exit::DAEMON_FAILED_SCENARIO,
    #[cfg(unix)]
    plan_exit::DAEMON_QUEUED_CYCLE_SCENARIO,
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
    // Custom multi-process scenario: see send_confirm::run_send_confirm_handoff.
    Scenario::new(
        send_confirm::SEND_CONFIRM_HANDOFF_ID,
        "Confirm a send whose conversation holds unreplayable native context",
        PtySize {
            rows: 16,
            cols: 100,
        },
        &[],
        /*smoke*/ false,
    ),
    codemode_stream::CODEMODE_CARD_SCENARIO,
    codemode_stream::CODEMODE_LARGE_SCRIPT_SCENARIO,
    #[cfg(unix)]
    codemode_stream::DAEMON_CODEMODE_LARGE_SCRIPT_SCENARIO,
    Scenario::new(
        "progress_tool",
        "Run the fixture progress tool to completion",
        DEFAULT_SIZE,
        PROGRESS_TOOL_STEPS,
        false,
    ),
    EDIT_DIFF_SCENARIO,
    todo_card::TODO_CARD_SCENARIO,
    todo_progress::TODO_PROGRESS_SCENARIO,
    write_stream::WRITE_INPUT_STREAM_SCENARIO,
    Scenario::new(
        "concurrent_progress",
        "Keep concurrent progress visible through out-of-order completion",
        DEFAULT_SIZE,
        CONCURRENT_PROGRESS_STEPS,
        false,
    ),
    STEER_APPEARS_IN_TRANSCRIPT_SCENARIO,
    STEER_DELIVERED_MARKER_SCENARIO,
    RETRACT_STEERING_DURING_TOOL_SCENARIO,
    QUEUE_FOLLOW_UP_DURING_TURN_SCENARIO,
    code_wrap::CODE_WRAP_GRAPHEME_SCENARIO,
    MARKDOWN_HEADINGS_SCENARIO,
    STREAMING_MARKDOWN_STABILITY_SCENARIO,
    SPINNER_ACTIVITY_ANCHOR_SCENARIO,
    SPINNER_ACTIVITY_JUMP_RAIL_SCENARIO,
    LIMITS_OVERLAY_SCENARIO,
    DOCTOR_OVERLAY_SCENARIO,
    DOCTOR_MCP_SCENARIO,
    spend::SPEND_OVERLAY_SCENARIO,
    context_overlay::CONTEXT_OVERLAY_SCENARIO,
    SIDE_OVERLAY_SCENARIO,
    SIDE_TOGGLE_SCENARIO,
    SIDE_BTW_SCENARIO,
    SIDE_DURING_TURN_SCENARIO,
    SIDE_COMPOSER_SCENARIO,
    fast_discovery::FAST_DISCOVERY_SCENARIO,
    #[cfg(unix)]
    fast_discovery::DAEMON_FAST_DISCOVERY_SCENARIO,
    #[cfg(unix)]
    fast_discovery::DAEMON_FAST_OFF_SCENARIO,
    #[cfg(unix)]
    command_palette::DAEMON_TAB_COMPLETE_SCENARIO,
    #[cfg(unix)]
    workspace_rewind::DAEMON_WORKSPACE_REWIND_OFF_SCENARIO,
    CREATE_AGENT_COMMAND_SCENARIO,
    CREATE_AGENT_MISSING_TOOLS_SCENARIO,
    #[cfg(unix)]
    command_palette::DAEMON_CREATE_AGENT_COMMAND_SCENARIO,
    #[cfg(unix)]
    command_palette::DAEMON_CREATE_AGENT_MISSING_TOOLS_SCENARIO,
    TAB_COMPLETE_ENTER_BARE_COMMAND_SCENARIO,
    PROMPT_TEMPLATE_HOT_RELOAD_SCENARIO,
    PROMPT_TEMPLATE_DURING_TURN_SCENARIO,
    FILE_PATH_AUTOCOMPLETE_SCENARIO,
    SHELL_TAB_COMPLETION_SCENARIO,
    Scenario::new(
        "mermaid_flowchart_resize",
        "Render a long-labelled flowchart, then explain the fallback in a narrow pane",
        DEFAULT_SIZE,
        MERMAID_FLOWCHART_RESIZE_STEPS,
        false,
    ),
    Scenario::new(
        "runtime_info",
        "Open runtime details immediately, copy the report, and keep fields readable after a narrow resize",
        PtySize {
            // Tall enough to show Workspace below the default auto-compaction rows.
            rows: 50,
            cols: 100,
        },
        RUNTIME_INFO_STEPS,
        false,
    )
    .with_env(&[("SSH_TTY", "rho-pty-clipboard")]),
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
    WORKSPACE_REWIND_OFF_SCENARIO,
    diff_viewer::DIFF_VIEWER_SCENARIO,
    copy_output::COPY_OUTPUT_SCENARIO,
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
        continue_latest::CONTINUE_LATEST_ID,
        "Continue the latest workspace session with -c, starting fresh when none exists",
        DEFAULT_SIZE,
        &[],
        /*smoke*/ false,
    ),
    #[cfg(unix)]
    daemon_resume::SCENARIO,
    #[cfg(unix)]
    daemon_detach::SCENARIO,
    #[cfg(unix)]
    daemon_context::SCENARIO,
    #[cfg(unix)]
    daemon_login::SCENARIO,
    #[cfg(unix)]
    daemon_reports::SCENARIO,
    #[cfg(unix)]
    daemon_tree::SCENARIO,
    #[cfg(unix)]
    daemon_rewind::SCENARIO,
    #[cfg(unix)]
    no_save::DAEMON_NO_SAVE_SESSION_SCENARIO,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    daemon_workflow::SCENARIO,
    #[cfg(unix)]
    daemon_switch::SCENARIO,
    #[cfg(unix)]
    daemon_subagents::AGENT_PROMPT_STREAMING,
    #[cfg(unix)]
    daemon_subagents::AGENT_MESSAGES,
    #[cfg(unix)]
    daemon_subagents::BACKGROUND_AGENT_AUTO_DELIVERY,
    #[cfg(unix)]
    daemon_subagents::BACKGROUND_AGENT_QUESTIONNAIRE,
    #[cfg(unix)]
    daemon_subagents::SUBAGENT_RAIL_MOUSE,
    #[cfg(unix)]
    daemon_subagents::QUIET_SUBAGENT_NOTICES,
    #[cfg(unix)]
    daemon_subagents::QUIET_SUBAGENT_RUNNING_NOTICES,
    #[cfg(unix)]
    daemon_subagents::STREAMING_BOUNDARY_NOTICE,
    #[cfg(unix)]
    daemon_subagents::SUBAGENT_COMPLETION_AFTER_NEW,
    #[cfg(unix)]
    daemon_subagents::ATTACH_PICKER,
    #[cfg(unix)]
    daemon_subagents::ATTACH_PARENT_MESSAGE,
    #[cfg(unix)]
    daemon_subagents::ATTACH_VIEW_FROM_COMMAND,
    #[cfg(unix)]
    daemon_subagents::ATTACH_VIEW_CYCLE,
    #[cfg(unix)]
    daemon_subagents::ATTACH_VIEW_PARENT_APPROVAL,
    #[cfg(unix)]
    daemon_subagents::ATTACH_VIEW_QUIT_RESTORES,
    #[cfg(unix)]
    daemon_restart::SCENARIO,
    #[cfg(unix)]
    daemon_goal::SCENARIO,
    #[cfg(unix)]
    daemon_mcp::SCENARIO,
    #[cfg(unix)]
    daemon_doctor::SCENARIO,
    #[cfg(unix)]
    daemon_prompt::DAEMON_PROMPT_SCENARIO,
    #[cfg(unix)]
    daemon_prompt::DAEMON_CANCEL_SCENARIO,
    #[cfg(unix)]
    daemon_prompt::DAEMON_STREAM_COST_SCENARIO,
    #[cfg(unix)]
    daemon_approval::DAEMON_APPROVAL_SCENARIO,
    #[cfg(unix)]
    daemon_approval::DAEMON_APPROVAL_DENY_SCENARIO,
    #[cfg(unix)]
    daemon_options::PERMISSION_MODE,
    #[cfg(unix)]
    daemon_model::OPEN_MODEL_PICKER,
    #[cfg(unix)]
    daemon_model::CYCLE_AND_PINNED_MODEL_PICKER,
    #[cfg(unix)]
    daemon_model::STATUSLINE_MODEL_CLICK,
    #[cfg(unix)]
    daemon_model::STATUSLINE_NO_REASONING_CLICK,
    #[cfg(unix)]
    daemon_model::MODEL_PROMPTS,
    #[cfg(unix)]
    daemon_model::MODEL_PROMPT_BEFORE_FIRST_PROMPT,
    #[cfg(unix)]
    daemon_model::CYCLE_REASONING_IDLE,
    #[cfg(unix)]
    daemon_model::CYCLE_REASONING_QUEUED,
    #[cfg(unix)]
    daemon_mcp_startup::CONNECTING,
    #[cfg(unix)]
    daemon_mcp_startup::HOLD_TAKE_BACK,
    #[cfg(unix)]
    daemon_mcp_startup::CONNECT_RELEASE,
    #[cfg(unix)]
    daemon_config::DAEMON_CONFIG,
    #[cfg(unix)]
    daemon_config::DAEMON_ADVISOR,
    Scenario::new(
        "sessions_hub",
        "Inspect a foreign session safely, then browse, resume, and delete locally",
        DEFAULT_SIZE,
        SESSIONS_HUB_STEPS,
        false,
    )
    .with_setup(setup_sessions_hub),
    sessions_tool::SESSIONS_TOOL_SCENARIO,
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
    model_prompts::MODEL_PROMPTS_SCENARIO,
    model_prompts::BEFORE_FIRST_PROMPT_SCENARIO,
    model_cycle_reasoning::IDLE,
    model_cycle_reasoning::QUEUED,
    config_2x_upgrade::IN_PROCESS,
    #[cfg(unix)]
    config_2x_upgrade::DAEMON,
    permission_cycle::IDLE,
    permission_cycle::REMAPPED,
    permission_cycle::QUEUED,
    #[cfg(unix)]
    permission_cycle::DAEMON_QUEUED,
    Scenario::new(
        "open_workflow_hub_empty",
        "Open the workflows hub when the workspace has no workflows yet",
        DEFAULT_SIZE,
        OPEN_WORKFLOW_HUB_EMPTY_STEPS,
        false,
    ),
    Scenario::new(
        WORKFLOW_HUB_UNSUPPORTED_ID,
        "List records saved before Rho 2.15 in the workflows hub and explain them without exiting",
        DEFAULT_SIZE,
        WORKFLOW_HUB_UNSUPPORTED_STEPS,
        false,
    )
    .with_setup(setup_workflow_hub_unsupported_records),
    Scenario::new(
        "open_agents_picker",
        "Browse agent metadata in a navigable popup and scroll hidden detail into view",
        DEFAULT_SIZE,
        OPEN_AGENTS_PICKER_STEPS,
        false,
    )
    .with_env(OPENAI_KEY_ENV),
    Scenario::new(
        "invalid_user_agent",
        "Start with an invalid user agent file and see it named in /agents",
        DEFAULT_SIZE,
        INVALID_USER_AGENT_STEPS,
        false,
    )
    .with_env(OPENAI_KEY_ENV)
    .with_setup(setup_invalid_user_agent),
    Scenario::new(
        "edit_user_agent",
        "Edit and save a user-defined agent through the agents picker",
        DEFAULT_SIZE,
        EDIT_USER_AGENT_STEPS,
        false,
    )
    .with_setup(setup_edit_user_agent),
    Scenario::new(
        "edit_user_agent_tools",
        "Toggle tools in the agent editor multi-select and persist the narrowed list",
        DEFAULT_SIZE,
        EDIT_USER_AGENT_TOOLS_STEPS,
        false,
    )
    .with_setup(setup_edit_user_agent),
    Scenario::new(
        "delete_user_agent",
        "Delete one of your agents from the agents picker after confirmation",
        DEFAULT_SIZE,
        DELETE_USER_AGENT_STEPS,
        false,
    )
    .with_setup(setup_edit_user_agent),
    PLUGIN_AGENT_MODEL_SCENARIO,
    Scenario::new(
        "view_read_only_agent_prompt",
        "Open a built-in agent's full prompt from the agents picker and return",
        DEFAULT_SIZE,
        VIEW_READ_ONLY_AGENT_PROMPT_STEPS,
        false,
    ),
    Scenario::new(
        "first_run_setup",
        "Walk a first launch through the full-screen sign-in and model steps",
        DEFAULT_SIZE,
        FIRST_RUN_SETUP_STEPS,
        /*smoke*/ false,
    )
    .with_env(FIRST_RUN_SIGNIN_ENV)
    .with_setup(first_run::setup_without_credential_store),
    Scenario::new(
        "first_run_decision_host",
        "Sign in to a decision-model host at first launch, then finish setup with a chat provider",
        DEFAULT_SIZE,
        FIRST_RUN_DECISION_HOST_STEPS,
        /*smoke*/ false,
    )
    .with_env(FIRST_RUN_SIGNIN_ENV)
    .with_setup(first_run::setup_without_credential_store),
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
    #[cfg(unix)]
    first_run::DAEMON_FIRST_RUN_SETUP_SCENARIO,
    #[cfg(unix)]
    first_run::DAEMON_SIGNED_OUT_SCENARIO,
    Scenario::new(
        "login_provider_groups",
        "Group login providers and open readable authentication methods",
        DEFAULT_SIZE,
        LOGIN_PROVIDER_GROUPS_STEPS,
        false,
    ),
    LOGIN_OAUTH_FLOW_CHOICE_SCENARIO,
    line_editor::LINE_EDITOR_SCENARIO,
    line_editor::SETUP_LINE_EDITOR_SCENARIO,
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
    #[cfg(unix)]
    quiet_subagent::QUIET_SUBAGENT_SCENARIO,
    #[cfg(unix)]
    quiet_subagent::GOAL_ACTION_RETRY_SCENARIO,
    #[cfg(unix)]
    quiet_subagent::RUNNING_NOTICES_SCENARIO,
    #[cfg(unix)]
    quiet_subagent::STREAMING_NOTICE_SCENARIO,
    #[cfg(unix)]
    quiet_subagent::COMPLETION_AFTER_NEW_SCENARIO,
    #[cfg(unix)]
    terminal_ownership::SHELL_KEEPS_TERMINAL_SCENARIO,
    new_session_prompt::SCENARIO,
    agent_messages::AGENT_MESSAGES_SCENARIO,
    boundary_notifications::SCENARIO,
    calibrated_context::SCENARIO,
    ATTACH_PICKER_SCENARIO,
    ATTACH_PARENT_MESSAGE_SCENARIO,
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
        "Update the drag selection highlight before release, copy transcript rows without gutters, and keep a recalled prompt on composer click",
        DEFAULT_SIZE,
        TEXT_SELECTION_DRAG_STEPS,
        false,
    )
    // Exercise OSC 52 on every host instead of the runner's native clipboard.
    .with_env(&[("SSH_TTY", "rho-pty-clipboard")]),
    Scenario::new(
        "screen_text_selection",
        "Click and drag in the composer to place the caret and replace selected text",
        DEFAULT_SIZE,
        SCREEN_TEXT_SELECTION_STEPS,
        false,
    ),
    ADVISOR_MISSING_MODEL_SCENARIO,
    ADVISOR_REVIEW_SCENARIO,
    Scenario::new(
        "late_background_agent_questionnaire",
        "Answer a questionnaire a background agent raises after its parent's turn ended",
        DEFAULT_SIZE,
        LATE_BACKGROUND_AGENT_QUESTIONNAIRE_STEPS,
        false,
    ),
    #[cfg(unix)]
    daemon_subagents::LATE_BACKGROUND_AGENT_QUESTIONNAIRE,
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

mod fixture_release;
use fixture_release::release_fixture;

pub use dispatch::run_named;
