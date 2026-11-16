//! A2UI surfaces the `ui` tool shows: drawn in the transcript and operated
//! from the keyboard.

use std::{fs::OpenOptions, io::Write};

use anyhow::Result;

use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

/// The hint under a surface the keyboard is on.
const FOCUSED: &str = "Tab move";

/// The hint under a surface the keyboard can move to.
const REACHABLE: &str = "Alt+U interact";

const FORM_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture surface"),
    Step::Phase("surface_takes_the_keyboard"),
    Step::WaitText {
        text: "Team signup",
        timeout: STREAM,
    },
    Step::WaitText {
        text: FOCUSED,
        timeout: STREAM,
    },
    Step::Phase("edit_each_control"),
    Step::Key(Key::Enter),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::WaitText {
        text: "a name is required",
        timeout: STREAM,
    },
    Step::Custom(type_grace),
    Step::Key(Key::Enter),
    Step::WaitTextGone {
        text: "a name is required",
        timeout: STREAM,
    },
    Step::Key(Key::Tab),
    Step::Key(Key::Char(' ')),
    Step::WaitText {
        text: "[x] Email me updates",
        timeout: STREAM,
    },
    Step::Key(Key::Tab),
    Step::Key(Key::Tab),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "(•) Pro",
        timeout: STREAM,
    },
    Step::Key(Key::Tab),
    Step::Key(Key::Right),
    Step::Phase("submit"),
    Step::Key(Key::Tab),
    Step::Key(Key::Tab),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: r#"surface call returned: submit name="Grace" notify=true plan=["pro"] seats=4"#,
        timeout: STREAM,
    },
    Step::Phase("keyboard_returns_to_the_composer"),
    Step::WaitText {
        text: REACHABLE,
        timeout: STREAM,
    },
    Step::ExitCommand,
];

fn type_grace(harness: &mut PtyHarness) -> Result<()> {
    harness.inject_key(&Key::Text("Grace".to_owned()))
}

pub(super) const FORM: Scenario = Scenario::new(
    "surface_form",
    "Edit a waiting ui call's form from the keyboard and submit it",
    DEFAULT_SIZE,
    FORM_STEPS,
    /*smoke*/ false,
);

fn typing_reached_the_composer(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("draft", STREAM)?;
    if harness.screen().contains_text("Adadraft") {
        anyhow::bail!(
            "typing after Esc went to the surface's field:\n{}",
            harness.screen().debug_dump()
        );
    }
    Ok(())
}

const ESCAPE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture surface"),
    Step::WaitText {
        text: FOCUSED,
        timeout: STREAM,
    },
    Step::Phase("esc_returns_to_the_composer"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: REACHABLE,
        timeout: STREAM,
    },
    Step::TypeText("draft"),
    Step::Custom(typing_reached_the_composer),
    Step::Phase("chord_returns_to_the_surface"),
    Step::Key(Key::Alt('u')),
    Step::WaitText {
        text: FOCUSED,
        timeout: STREAM,
    },
    // From the first control, back past the last one to Cancel.
    Step::Key(Key::Up),
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "surface call returned: cancel",
        timeout: STREAM,
    },
    Step::Phase("draft_survived"),
    Step::AssertText("> draft"),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::WaitTextGone {
        text: "> draft",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const ESCAPE: Scenario = Scenario::new(
    "surface_escape",
    "Esc leaves a surface for the composer and Alt+U returns to it",
    DEFAULT_SIZE,
    ESCAPE_STEPS,
    /*smoke*/ false,
);

const IDLE_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture surface gallery"),
    Step::Phase("returned_call_leaves_the_composer_alone"),
    Step::WaitText {
        text: "surface call returned",
        timeout: STREAM,
    },
    Step::WaitText {
        text: REACHABLE,
        timeout: STREAM,
    },
    Step::Phase("operate_the_surface"),
    Step::Key(Key::Alt('u')),
    Step::WaitText {
        text: "Tab move  Left/Right change",
        timeout: STREAM,
    },
    Step::Key(Key::Right),
    Step::WaitText {
        text: "Architecture diagram",
        timeout: STREAM,
    },
    // From the first control, back past the last one, the Apply button.
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: r#"surface action delivered: apply tags=["rust"] when="2026-01-31""#,
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const IDLE: Scenario = Scenario::new(
    "surface_idle_action",
    "Act on a surface whose call returned; the idle session wakes to read it",
    DEFAULT_SIZE,
    IDLE_STEPS,
    /*smoke*/ false,
);

fn drawn_once_after_reset(harness: &mut PtyHarness) -> Result<()> {
    harness.wait_for_text("Component gallery", STREAM)?;
    harness.wait_for_text(REACHABLE, STREAM)?;
    let drawn = harness
        .screen()
        .contents()
        .matches("Component gallery")
        .count();
    anyhow::ensure!(
        drawn == 1,
        "the replay drew the surface {drawn} times:\n{}",
        harness.screen().debug_dump()
    );
    Ok(())
}

const RESET_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture surface gallery"),
    Step::WaitText {
        text: REACHABLE,
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Type a message",
        timeout: STREAM,
    },
    Step::SubmitText("hello second"),
    Step::WaitText {
        text: "fixture response: hello second",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Type a message",
        timeout: STREAM,
    },
    Step::Phase("restore_the_first_turn"),
    Step::SubmitText("/tree"),
    Step::WaitText {
        text: "Conversation tree",
        timeout: STREAM,
    },
    // The picker opens on the active turn, the second.
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitTextGone {
        text: "fixture response: hello second",
        timeout: STREAM,
    },
    Step::Custom(drawn_once_after_reset),
    Step::Phase("replayed_surface_still_acts"),
    Step::Key(Key::Alt('u')),
    Step::Key(Key::Right),
    Step::WaitText {
        text: "Architecture diagram",
        timeout: STREAM,
    },
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "surface action delivered: apply",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const RESET: Scenario = Scenario::new(
    "surface_reset",
    "A reset replay rebuilds a surface once, and it still takes actions",
    DEFAULT_SIZE,
    RESET_STEPS,
    /*smoke*/ false,
);

fn setup_zen(home: &IsolatedHome) -> Result<()> {
    writeln!(
        OpenOptions::new()
            .append(true)
            .open(&home.client_settings_path)?,
        "zen_mode = true"
    )?;
    Ok(())
}

const ZEN_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("fixture surface"),
    Step::Phase("waiting_surface_shows_without_its_card"),
    Step::WaitText {
        text: "Team signup",
        timeout: STREAM,
    },
    Step::WaitText {
        text: FOCUSED,
        timeout: STREAM,
    },
    Step::Key(Key::Up),
    Step::Key(Key::Up),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "surface call returned: cancel",
        timeout: STREAM,
    },
    Step::Phase("returned_surface_stays"),
    Step::WaitText {
        text: REACHABLE,
        timeout: STREAM,
    },
    Step::AssertText("Team signup"),
    Step::ExitCommand,
];

pub(super) const ZEN: Scenario = Scenario::new(
    "surface_zen",
    "Zen mode hides the ui call's card and keeps its surface",
    DEFAULT_SIZE,
    ZEN_STEPS,
    /*smoke*/ false,
)
.with_setup(setup_zen);
