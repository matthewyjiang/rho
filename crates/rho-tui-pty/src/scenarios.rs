//! Built-in Rho TUI PTY scenarios.

use std::time::Duration;

use anyhow::Result;

use crate::{
    harness::WaitTimeout,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, ScenarioOutcome, ScenarioRunner, Step},
};

/// Stable scenario identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioId {
    StartupStreamExit,
    CancelAndResubmit,
    ResizeDuringStream,
    ScrollDuringStream,
    TerminalRestoration,
    PasteMultiline,
    Questionnaire,
    ProgressTool,
    MarkdownHeadings,
    OpenModelPicker,
}

impl ScenarioId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StartupStreamExit => "startup_stream_exit",
            Self::CancelAndResubmit => "cancel_and_resubmit",
            Self::ResizeDuringStream => "resize_during_stream",
            Self::ScrollDuringStream => "scroll_during_stream",
            Self::TerminalRestoration => "terminal_restoration",
            Self::PasteMultiline => "paste_multiline",
            Self::Questionnaire => "questionnaire",
            Self::ProgressTool => "progress_tool",
            Self::MarkdownHeadings => "markdown_headings",
            Self::OpenModelPicker => "open_model_picker",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "startup_stream_exit" => Some(Self::StartupStreamExit),
            "cancel_and_resubmit" => Some(Self::CancelAndResubmit),
            "resize_during_stream" => Some(Self::ResizeDuringStream),
            "scroll_during_stream" => Some(Self::ScrollDuringStream),
            "terminal_restoration" => Some(Self::TerminalRestoration),
            "paste_multiline" => Some(Self::PasteMultiline),
            "questionnaire" => Some(Self::Questionnaire),
            "progress_tool" => Some(Self::ProgressTool),
            "markdown_headings" => Some(Self::MarkdownHeadings),
            "open_model_picker" => Some(Self::OpenModelPicker),
            _ => None,
        }
    }
}

const DEFAULT_SIZE: PtySize = PtySize {
    rows: 28,
    cols: 100,
};

const STARTUP: WaitTimeout = WaitTimeout::secs(20, "startup");
const STREAM: WaitTimeout = WaitTimeout::secs(20, "stream response");
const SETTLE: WaitTimeout = WaitTimeout::secs(10, "ui settle");

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
    Step::Phase("bulk"),
    Step::SubmitText("fixture bulk one"),
    // Bulk output is long; the viewport follows the bottom, so assert a late line.
    Step::WaitText {
        text: "fixture bulk one line 180",
        timeout: STREAM,
    },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(200),
        timeout: WaitTimeout::secs(15, "bulk settle"),
    },
    Step::Phase("scroll_up"),
    Step::Key(Key::PageUp),
    Step::Key(Key::PageUp),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Phase("return_bottom"),
    Step::Key(Key::Ctrl('g')),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
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
    Step::Phase("paste"),
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

const OPEN_MODEL_PICKER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/model"),
    Step::WaitText {
        text: "select model",
        timeout: STARTUP,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

/// All registered scenarios.
pub fn all_scenarios() -> &'static [Scenario] {
    &[
        Scenario {
            id: "startup_stream_exit",
            description: "Start, stream a fixture response, and exit cleanly",
            size: DEFAULT_SIZE,
            steps: STARTUP_STREAM_EXIT_STEPS,
            smoke: true,
        },
        Scenario {
            id: "cancel_and_resubmit",
            description: "Cancel a long fixture stream and submit another prompt",
            size: DEFAULT_SIZE,
            steps: CANCEL_AND_RESUBMIT_STEPS,
            smoke: true,
        },
        Scenario {
            id: "resize_during_stream",
            description: "Resize repeatedly while a fixture stream is active",
            size: DEFAULT_SIZE,
            steps: RESIZE_DURING_STREAM_STEPS,
            smoke: true,
        },
        Scenario {
            id: "scroll_during_stream",
            description: "Scroll during bulk output and return to bottom",
            size: DEFAULT_SIZE,
            steps: SCROLL_DURING_STREAM_STEPS,
            smoke: true,
        },
        Scenario {
            id: "terminal_restoration",
            description: "Verify alternate-screen enter/leave around a clean exit",
            size: DEFAULT_SIZE,
            steps: TERMINAL_RESTORATION_STEPS,
            smoke: true,
        },
        Scenario {
            id: "paste_multiline",
            description: "Paste multiline text without treating embedded lines as commands",
            size: DEFAULT_SIZE,
            steps: PASTE_MULTILINE_STEPS,
            smoke: false,
        },
        Scenario {
            id: "questionnaire",
            description: "Exercise questionnaire keyboard selection and submission",
            size: DEFAULT_SIZE,
            steps: QUESTIONNAIRE_STEPS,
            smoke: false,
        },
        Scenario {
            id: "progress_tool",
            description: "Run the fixture progress tool to completion",
            size: DEFAULT_SIZE,
            steps: PROGRESS_TOOL_STEPS,
            smoke: false,
        },
        Scenario {
            id: "markdown_headings",
            description: "Render streamed Markdown heading levels without syntax markers",
            size: DEFAULT_SIZE,
            steps: MARKDOWN_HEADINGS_STEPS,
            smoke: false,
        },
        Scenario {
            id: "open_model_picker",
            description: "Open and dismiss the model picker",
            size: DEFAULT_SIZE,
            steps: OPEN_MODEL_PICKER_STEPS,
            smoke: false,
        },
    ]
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
