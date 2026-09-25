//! Questionnaire and approval choices: keyboard and pointer selection and
//! confirmation.

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    harness::WaitTimeout,
    keys::{Key, MouseButton},
    scenario::{Scenario, Step},
    PtyHarness,
};

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

pub(super) const QUESTIONNAIRE_SCENARIO: Scenario = Scenario::new(
    "questionnaire",
    "Exercise questionnaire keyboard selection and submission",
    DEFAULT_SIZE,
    QUESTIONNAIRE_STEPS,
    /*smoke*/ false,
);

const CLICK: WaitTimeout = WaitTimeout::secs(5, "choice click");

/// 1-based SGR cell of the lowest on-screen occurrence of `needle`. The
/// composer sits below the transcript, whose tool cards can echo choice labels.
fn click_cell(harness: &PtyHarness, needle: &str) -> Result<(u16, u16)> {
    for (row, line) in harness.screen().rows_text().iter().enumerate().rev() {
        if let Some(offset) = line.find(needle) {
            let column = UnicodeWidthStr::width(&line[..offset]);
            return Ok((column as u16 + 1, row as u16 + 1));
        }
    }
    anyhow::bail!("'{needle}' not found:\n{}", harness.screen().debug_dump());
}

fn click(harness: &mut PtyHarness, (column, row): (u16, u16)) -> Result<()> {
    harness.mouse(MouseButton::Left, column, row, true)?;
    harness.mouse(MouseButton::Left, column, row, false)
}

/// Two clicks on one cell. Moving away first ends any earlier click sequence,
/// so a single click just before cannot pair with the first of these.
fn double_click(harness: &mut PtyHarness, cell: (u16, u16)) -> Result<()> {
    harness.mouse_move(1, 1)?;
    click(harness, cell)?;
    click(harness, cell)
}

// Covers: one click moves the questionnaire selection without submitting, and
// a double click confirms like Enter.
// Owner: interactive UX (PTY).
fn click_then_double_click_questionnaire_choice(harness: &mut PtyHarness) -> Result<()> {
    let blue = click_cell(harness, "blue")?;
    click(harness, blue)?;
    harness.wait_for_text("→ ● blue", CLICK)?;
    if harness
        .screen()
        .contains_text("questionnaire response observed")
    {
        anyhow::bail!("a single click submitted the questionnaire");
    }
    double_click(harness, blue)?;
    harness.wait_for_text("questionnaire response observed exactly 1 time", STREAM)?;
    harness.wait_for_text("\"answer\":\"blue\"", STREAM)
}

const QUESTIONNAIRE_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture questionnaire"),
    Step::WaitText {
        text: "A cool primary color",
        timeout: STREAM,
    },
    Step::Phase("click_choice"),
    Step::Custom(click_then_double_click_questionnaire_choice),
    Step::ExitCommand,
];

fn setup_supervised(home: &IsolatedHome) -> Result<()> {
    let config = std::fs::read_to_string(&home.config_path)?;
    // Top-level keys must precede the first table header.
    std::fs::write(
        &home.config_path,
        format!("permission_mode = \"supervised\"\n{config}"),
    )?;
    Ok(())
}

// Covers: a single click only moves the approval highlight and never resolves
// it; a double click resolves it and resumes the turn, the same path as Enter.
// Owner: interactive UX (PTY).
fn double_click_allow_once(harness: &mut PtyHarness) -> Result<()> {
    let allow = click_cell(harness, "Allow once")?;
    click(harness, allow)?;
    // The highlight repaint follows the click, so the prompt state is settled.
    harness.wait_for_text("→ Allow once", CLICK)?;
    let screen = harness.screen();
    if !screen.contains_text("Allow for session")
        || screen.contains_text("reviewing harmless fixturesegment-01")
    {
        anyhow::bail!(
            "a single click resolved the approval:\n{}",
            screen.debug_dump()
        );
    }
    double_click(harness, allow)?;
    // Joined printf output only exists if the command actually ran; the
    // follow-up fixture reply would also appear after a denial.
    harness.wait_for_text("reviewing harmless fixturesegment-01", STREAM)?;
    harness.wait_for_text("fixture response: fixture approval long", STREAM)
}

const APPROVAL_CLICK_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture approval long"),
    Step::WaitText {
        text: "Allow for session",
        timeout: STREAM,
    },
    Step::Phase("click_allow"),
    Step::Custom(double_click_allow_once),
    Step::ExitCommand,
];

pub(super) const QUESTIONNAIRE_CLICK_SCENARIO: Scenario = Scenario::new(
    "questionnaire_click",
    "Select a questionnaire choice with a click and submit with a double click",
    DEFAULT_SIZE,
    QUESTIONNAIRE_CLICK_STEPS,
    /*smoke*/ false,
);

pub(super) const APPROVAL_CLICK_SCENARIO: Scenario = Scenario::new(
    "approval_click",
    "Allow a supervised approval with a double click and resume the turn",
    DEFAULT_SIZE,
    APPROVAL_CLICK_STEPS,
    /*smoke*/ false,
)
.with_setup(setup_supervised);
