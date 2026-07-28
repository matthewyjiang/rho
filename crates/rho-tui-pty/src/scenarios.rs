//! Built-in Rho TUI PTY scenarios.

mod config;
mod conversation_tree;
mod goal;
mod login;
mod mermaid;
mod pickers;
mod resume_delete;
mod runtime_info;
mod subagent_rail;

use config::OPEN_CONFIG_PICKER_STEPS;
use conversation_tree::CONVERSATION_TREE_STEPS;
use goal::{
    GOAL_BLOCKED_AND_RESUMED_STEPS, GOAL_QUESTIONNAIRE_STEPS,
    GOAL_WAITS_FOR_SUBAGENTS_DURING_RETRY_STEPS, GOAL_WAITS_FOR_SUBAGENTS_STEPS,
};
use login::LOGIN_PROVIDER_GROUPS_STEPS;
use mermaid::MERMAID_FLOWCHART_RESIZE_STEPS;
use pickers::{
    setup_edit_user_agent, EDIT_USER_AGENT_STEPS, OPEN_AGENTS_PICKER_STEPS, OPEN_MODEL_PICKER_STEPS,
};
use resume_delete::RESUME_PICKER_DELETE_STEPS;
use runtime_info::RUNTIME_INFO_STEPS;
use std::time::Duration;
use subagent_rail::SUBAGENT_RAIL_MOUSE_STEPS;

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
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
pub(super) const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

const STARTUP_STREAM_EXIT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "rho",
        timeout: STARTUP,
    },
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("submit_stream"),
    Step::SubmitText("fixture stream"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
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

const TYPE_DURING_STREAM_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_flood"),
    Step::SubmitText("fixture input flood"),
    Step::WaitText {
        text: "input flood event 010",
        timeout: STREAM,
    },
    Step::Phase("query_limits"),
    Step::SubmitText("/limits"),
    Step::WaitText {
        text: "no supported OAuth providers are connected",
        timeout: STREAM,
    },
    Step::Phase("type_draft"),
    Step::TypeText("draft while streaming"),
    Step::WaitText {
        text: "draft while streaming",
        timeout: WaitTimeout::secs(2, "composer input during stream"),
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('c')),
    Step::ExitCommand,
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

const PASTE_MULTILINE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("delete_collapsed_paste"),
    Step::Paste("discard one\ndiscard two\ndiscard three"),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Key(Key::Backspace),
    Step::SubmitText("fixture stream"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "part two",
        timeout: STREAM,
    },
    Step::Phase("submit_multiline_paste"),
    Step::Paste("line one\n/not-a-command\nline three"),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture response:",
        timeout: STREAM,
    },
    Step::ExitCommand,
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

const SUPERVISED_APPROVAL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("enable_supervised_mode"),
    Step::SubmitText("/config"),
    Step::WaitText {
        text: "Agent behavior",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    Step::Key(Key::Enter),
    // Nested mode list keeps every mode label visible; only the detail pane
    // shows the selected mode's description.
    Step::WaitText {
        text: "No permission checks",
        timeout: SETTLE,
    },
    Step::AssertText("Auto"),
    Step::AssertText("Plan"),
    Step::AssertText("Supervised"),
    Step::Key(Key::Down),
    Step::Key(Key::Down),
    Step::WaitText {
        text: "Ask before writes and processes",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    // Selection returns to the agent-behavior category with the label badge.
    Step::WaitText {
        text: "Config / Agent behavior",
        timeout: SETTLE,
    },
    Step::AssertText("Permission mode"),
    Step::AssertText("Supervised"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Models & reasoning",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "permissions: supervised",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "permission mode: supervised",
        timeout: SETTLE,
    },
    Step::Phase("inspect_long_process_approval"),
    Step::SubmitText("fixture approval long"),
    Step::WaitText {
        text: "bash wants to execute",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "DANGEROUS_SUFFIX_INSPECTABLE",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "Allow for session",
        timeout: SETTLE,
    },
    Step::WaitText {
        text: "pgup/pgdn details",
        timeout: SETTLE,
    },
    Step::Key(Key::PageUp),
    Step::WaitText {
        text: "output limit:",
        timeout: SETTLE,
    },
    Step::Key(Key::PageDown),
    Step::WaitText {
        text: "DANGEROUS_SUFFIX_INSPECTABLE",
        timeout: SETTLE,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "model interrupted",
        timeout: STREAM,
    },
    Step::Phase("continue_session"),
    Step::SubmitText("fixture stream"),
    Step::WaitText {
        text: "assistant stream part one",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "part two",
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

const RETRACT_STEERING_DURING_TOOL_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("start_tool"),
    Step::SubmitText("fixture progress tool"),
    Step::WaitText {
        text: "deterministic progress update one",
        timeout: STREAM,
    },
    Step::Phase("steer"),
    Step::SubmitText("keep the public API unchanged"),
    Step::WaitText {
        text: "pending input",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "STEER",
        timeout: STREAM,
    },
    Step::Phase("retract"),
    Step::Key(Key::AltUp),
    Step::WaitText {
        text: "editing retracted steer",
        timeout: STREAM,
    },
    Step::Key(Key::Ctrl('c')),
    Step::WaitText {
        text: "input cleared",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "progress tool lifecycle complete",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

const MARKDOWN_HEADINGS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture markdown headings"),
    Step::WaitText {
        text: "Level six",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: SETTLE,
    },
    Step::Custom(assert_markdown_headings_rendered),
    Step::ExitCommand,
];

fn assert_agent_tool_hides_raw_json(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("\"agent_id\"")
        || screen.contains("\"background\":true")
        || screen.contains("\"action\":\"list\"")
    {
        anyhow::bail!("agent tool exposed raw JSON:\n{screen}");
    }
    Ok(())
}

const BACKGROUND_AGENT_AUTO_DELIVERY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_agent"),
    Step::SubmitText("fixture background agent"),
    Step::WaitText {
        text: "● wor  starting",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "● worker  running in background",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "fixture stream",
        timeout: STREAM,
    },
    Step::Custom(assert_agent_tool_hides_raw_json),
    // The fixture echoes the spawn receipt's first line, proving the tool
    // resolved with a start line and the parent turn ended.
    Step::WaitText {
        text: "background agent dispatched: agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "(worker) started in background",
        timeout: STREAM,
    },
    Step::Phase("automatic_completion_delivery"),
    // The fixture validates the notification's real payload (agent identity,
    // terminal state, delegated result) and counts notification turns, so
    // this asserts a well-formed, exactly-once delivery.
    Step::WaitText {
        text: "background agent completion received with delegated result (delivery 1)",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::Phase("list_agents"),
    Step::SubmitText("fixture agents list"),
    Step::WaitText {
        text: "delegated agents",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "worker  completed",
        timeout: STREAM,
    },
    Step::Custom(assert_agent_tool_hides_raw_json),
    Step::ExitCommand,
];

fn assert_background_questionnaire_parent_active(harness: &mut crate::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if screen.contains("background questionnaire agent dispatched: agent") {
        anyhow::bail!("parent turn ended before the child questionnaire appeared:\n{screen}");
    }
    Ok(())
}

const BACKGROUND_AGENT_QUESTIONNAIRE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("spawn_background_questionnaire_agent"),
    Step::SubmitText("fixture background questionnaire"),
    Step::Phase("answer_child_questionnaire"),
    Step::WaitText {
        text: "asks: Background questionnaire",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Choose one color",
        timeout: STREAM,
    },
    Step::Custom(assert_background_questionnaire_parent_active),
    Step::Phase("parent_finishes_while_questionnaire_remains_open"),
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    Step::Key(Key::Down),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "answered questionnaire for agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "background questionnaire agent dispatched: agent",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "(worker) started in background",
        timeout: STREAM,
    },
    Step::Phase("automatic_completion_delivery"),
    Step::WaitText {
        text: "background agent questionnaire completion received (delivery 1)",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(250),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

/// All registered scenarios.
const ALL_SCENARIOS: &[Scenario] = &[
    Scenario::new(
        "startup_stream_exit",
        "Start, stream a fixture response, and exit cleanly",
        DEFAULT_SIZE,
        STARTUP_STREAM_EXIT_STEPS,
        true,
    ),
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
        "Keep composer input responsive during continuous model output",
        DEFAULT_SIZE,
        TYPE_DURING_STREAM_STEPS,
        true,
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
        "paste_multiline",
        "Paste multiline text without treating embedded lines as commands",
        DEFAULT_SIZE,
        PASTE_MULTILINE_STEPS,
        false,
    ),
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
        "progress_tool",
        "Run the fixture progress tool to completion",
        DEFAULT_SIZE,
        PROGRESS_TOOL_STEPS,
        false,
    ),
    Scenario::new(
        "concurrent_progress",
        "Keep concurrent progress visible through out-of-order completion",
        DEFAULT_SIZE,
        CONCURRENT_PROGRESS_STEPS,
        false,
    ),
    Scenario::new(
        "retract_steering_during_tool",
        "Inspect and retract steering while a tool is running",
        DEFAULT_SIZE,
        RETRACT_STEERING_DURING_TOOL_STEPS,
        true,
    ),
    Scenario::new(
        "markdown_headings",
        "Render streamed Markdown heading levels without syntax markers",
        DEFAULT_SIZE,
        MARKDOWN_HEADINGS_STEPS,
        false,
    ),
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
        "conversation_tree",
        "Restore an earlier turn and continue on a new branch",
        DEFAULT_SIZE,
        CONVERSATION_TREE_STEPS,
        false,
    ),
    Scenario::new(
        "resume_picker_delete",
        "Delete a saved session from the resume picker with confirm/cancel",
        DEFAULT_SIZE,
        RESUME_PICKER_DELETE_STEPS,
        false,
    ),
    Scenario::new(
        "open_model_picker",
        "Open and dismiss the model picker",
        DEFAULT_SIZE,
        OPEN_MODEL_PICKER_STEPS,
        false,
    ),
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
    ),
    Scenario {
        id: "edit_user_agent",
        description: "Edit and save a user-defined agent through the agents picker",
        size: DEFAULT_SIZE,
        setup: Some(setup_edit_user_agent),
        steps: EDIT_USER_AGENT_STEPS,
        smoke: false,
    },
    Scenario::new(
        "login_provider_groups",
        "Group login providers and open readable authentication methods",
        DEFAULT_SIZE,
        LOGIN_PROVIDER_GROUPS_STEPS,
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
    Scenario::new(
        "subagent_rail_mouse",
        "Keep hover through refreshes and activate rows on a completed click",
        DEFAULT_SIZE,
        SUBAGENT_RAIL_MOUSE_STEPS,
        false,
    ),
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

pub fn run_named(runner: &ScenarioRunner, name: &str) -> Result<ScenarioOutcome> {
    let scenario = all_scenarios()
        .iter()
        .find(|scenario| scenario.id == name)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario '{name}'"))?;
    runner.run(scenario)
}

fn assert_inline_shell_cancelled(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    if harness.screen().contains_text("cancel-escaped-output") {
        anyhow::bail!("inline shell produced output after Escape cancelled it");
    }
    Ok(())
}

fn assert_idle_shell_still_streaming(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    if harness.screen().contains_text("idle-stream-end") {
        anyhow::bail!("idle shell output was not rendered until the command completed");
    }
    Ok(())
}

fn assert_markdown_headings_rendered(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    for heading in [
        "Level one",
        "Level two",
        "Level three",
        "Level four",
        "Level five",
        "Level six",
    ] {
        if !screen.contains(heading) {
            anyhow::bail!("rendered heading is missing from the screen: {heading}");
        }
    }
    if screen
        .lines()
        .any(|line| line.trim_start().starts_with('#'))
    {
        anyhow::bail!("rendered heading retained Markdown syntax markers");
    }
    Ok(())
}

fn assert_terminal_restored(harness: &mut crate::harness::PtyHarness) -> Result<()> {
    // After a clean exit, ratatui/crossterm must leave the alternate screen.
    // Mouse disable alone is not enough: a regression that skips ESC[?1049l
    // would leave the user stuck in the alternate screen.
    let raw = harness.raw_output();
    let left = raw.windows(8).any(|window| window == b"\x1b[?1049l")
        || String::from_utf8_lossy(raw).contains("?1049l");
    if !left {
        anyhow::bail!("did not observe alternate-screen leave sequence (ESC[?1049l)");
    }
    Ok(())
}
