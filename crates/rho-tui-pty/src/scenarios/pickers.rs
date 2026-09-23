use std::time::Duration;

use anyhow::Result;

use crate::{env::IsolatedHome, keys::Key, scenario::Step, PtyHarness};

use super::{SETTLE, STARTUP};

/// Model pickers list models only for authenticated providers, so inject a
/// fixture key the way the advisor scenarios do.
pub(super) const OPENAI_KEY_ENV: &[(&str, &str)] = &[("OPENAI_API_KEY", "fixture-openai-key")];
pub(super) const OPENAI_AND_XAI_KEY_ENV: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "fixture-openai-key"),
    ("XAI_API_KEY", "fixture-xai-key"),
];

pub(super) const OPEN_MODEL_PICKER_STEPS: &[Step] = &[
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

/// Two authenticated providers and two pins: cycle from the composer, then
/// open /model on the pinned list and flip to all with Ctrl-O.
pub(super) fn setup_pinned_models(home: &IsolatedHome) -> Result<()> {
    std::fs::write(
        &home.config_path,
        r#"provider = "openai"
model = "gpt-5.5"
auth = "api-key"
check_for_updates = false
web_search.mode = "off"
favorite_models = ["openai/gpt-5.5", "xai/grok-4.6"]

[behavior]
credential_store = "file"
"#,
    )?;
    Ok(())
}

pub(super) const CYCLE_AND_PINNED_MODEL_PICKER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::Phase("cycle_forward"),
    Step::Key(Key::Ctrl('p')),
    Step::WaitText {
        text: "xai/grok-4.6",
        timeout: SETTLE,
    },
    // Wraps back to the first pin, proving the cycle is a ring and not a
    // one-shot jump to the next entry.
    Step::Phase("cycle_wraps"),
    Step::Key(Key::Ctrl('p')),
    Step::WaitText {
        text: "openai/gpt-5.5",
        timeout: SETTLE,
    },
    Step::Phase("open_pinned_picker"),
    Step::SubmitText("/model"),
    Step::WaitText {
        text: "select model · pinned",
        timeout: STARTUP,
    },
    Step::AssertText("openai/gpt-5.5"),
    Step::AssertText("xai/grok-4.6"),
    Step::Phase("toggle_all"),
    Step::Key(Key::Ctrl('o')),
    Step::WaitText {
        text: "select model · all",
        timeout: SETTLE,
    },
    // Back to pinned, then unpin both rows: the list must fall back to the
    // catalogue rather than stranding the user on an empty pinned view.
    Step::Phase("unpin_falls_back_to_all"),
    Step::Key(Key::Ctrl('o')),
    Step::WaitText {
        text: "select model · pinned",
        timeout: SETTLE,
    },
    Step::Key(Key::Ctrl('p')),
    Step::Key(Key::Ctrl('p')),
    Step::WaitText {
        text: "select model · all",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

/// Empty workspace must still open the workflows overlay instead of a chat notice.
pub(super) const OPEN_WORKFLOW_HUB_EMPTY_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/workflow"),
    Step::WaitText {
        text: "WORKFLOWS",
        timeout: STARTUP,
    },
    Step::AssertText("START"),
    Step::AssertText("RUNS"),
    Step::Key(Key::Esc),
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::ExitCommand,
];

pub(super) fn setup_edit_user_agent(home: &IsolatedHome) -> Result<()> {
    let agents = home.home.join(".rho/agents");
    std::fs::create_dir_all(&agents)?;
    std::fs::write(
        agents.join("editable-fixture.md"),
        "---\nid: editable-fixture\ndescription: fixture agent\n---\nfixture prompt\n",
    )?;
    Ok(())
}

pub(super) const EDIT_USER_AGENT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/agents"),
    Step::WaitText {
        text: "● editable",
        timeout: SETTLE,
    },
    Step::TypeText("editable-fixture"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit agent editable-fixture",
        timeout: SETTLE,
    },
    Step::AssertText("Description"),
    Step::AssertText("Save"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture agent",
        timeout: SETTLE,
    },
    Step::TypeText(" updated"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit agent editable-fixture",
        timeout: SETTLE,
    },
    Step::TypeText("Save"),
    Step::WaitText {
        text: "Serialize, validate",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "fixture agent updated",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit agent editable-fixture",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Loaded agents",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];

/// Enter on a read-only agent opens its full prompt in a panel; Esc returns
/// to the agents picker. The fact sheet only shows a short excerpt, so this
/// is the one place a built-in's full prompt is readable.
pub(super) const VIEW_READ_ONLY_AGENT_PROMPT_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/agents"),
    Step::WaitText {
        text: "BUILT IN",
        timeout: SETTLE,
    },
    Step::TypeText("reviewer"),
    Step::WaitText {
        text: "● read-only",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "reviewer prompt",
        timeout: SETTLE,
    },
    // Past the three-row excerpt: only the full view reaches this sentence.
    Step::AssertText("Do not modify files."),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Loaded agents",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];

/// Tools is a multi-select: Space toggles a row and the picker stays open, Esc
/// returns to the field list with the toggled draft, and Save persists it.
/// Starts from `tools: all`, so removing `shell` must expand the policy to the
/// explicit built-in set minus shell; toggling `all` on and back off must
/// return to that set rather than leave every tool on.
pub(super) const EDIT_USER_AGENT_TOOLS_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/agents"),
    Step::WaitText {
        text: "● editable",
        timeout: SETTLE,
    },
    Step::TypeText("editable-fixture"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit agent editable-fixture",
        timeout: SETTLE,
    },
    Step::TypeText("Tools"),
    Step::WaitText {
        text: "Rho tool capabilities",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Every host tool",
        timeout: SETTLE,
    },
    // Regex filter: `^shell` skips powershell.
    Step::TypeText("^shell"),
    Step::WaitText {
        text: "Run shell commands.",
        timeout: SETTLE,
    },
    Step::Key(Key::Char(' ')),
    // `all` is a toggle: on replaces the narrowed set, off restores it, so the
    // saved policy below must still be the explicit list minus shell.
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::TypeText("^all"),
    Step::WaitText {
        text: "Every host tool",
        timeout: SETTLE,
    },
    Step::Key(Key::Char(' ')),
    Step::WaitText {
        text: "tools: all",
        timeout: SETTLE,
    },
    Step::Key(Key::Char(' ')),
    Step::WaitText {
        text: "tools: advisor",
        timeout: SETTLE,
    },
    // The picker stays open after a toggle; Esc lands on the field list with
    // the "Tools" filter restored, so its badge is the durable outcome.
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Rho tool capabilities",
        timeout: SETTLE,
    },
    Step::Custom(assert_tools_badge_excludes_shell),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::Key(Key::Backspace),
    Step::TypeText("Save"),
    Step::WaitText {
        text: "Serialize, validate",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "agent editable-fixture saved",
        timeout: SETTLE,
    },
    // Reopen from disk: the Tools badge must show the narrowed explicit list.
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "edit agent editable-fixture",
        timeout: SETTLE,
    },
    Step::TypeText("Tools"),
    Step::WaitText {
        text: "Rho tool capabilities",
        timeout: SETTLE,
    },
    Step::Custom(assert_tools_badge_excludes_shell),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Loaded agents",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];

/// The Tools row detail pane lists the current allow list after "Current".
/// Toggling `shell` off from `all` must leave the other built-ins and drop
/// `shell` (but keep `powershell`, which merely contains the substring).
fn assert_tools_badge_excludes_shell(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let detail: Vec<&str> = screen
        .lines()
        .skip_while(|line| !line.contains("Current"))
        .skip(1)
        .take_while(|line| line.contains('│'))
        .collect();
    let names: Vec<&str> = detail
        .iter()
        .flat_map(|line| line.split(['│', ',', ' ']))
        .filter(|token| !token.is_empty())
        .collect();
    if !names.contains(&"read_file") || !names.contains(&"powershell") {
        anyhow::bail!("Tools detail did not list explicit capabilities:\n{screen}");
    }
    if names.contains(&"shell") {
        anyhow::bail!("Tools detail still lists shell after toggling it off:\n{screen}");
    }
    Ok(())
}

fn assert_wide_popup_divider_is_stable(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    let divider_columns = screen
        .lines()
        .filter_map(|line| {
            let dividers = line.match_indices('│').collect::<Vec<_>>();
            (dividers.len() >= 3).then(|| line[..dividers[1].0].chars().count())
        })
        .collect::<Vec<_>>();
    if divider_columns.len() < 10 {
        anyhow::bail!("agents popup divider was missing from body rows:\n{screen}");
    }
    if !divider_columns
        .iter()
        .all(|column| *column == divider_columns[0])
    {
        anyhow::bail!("agents popup divider shifted between rows:\n{screen}");
    }
    Ok(())
}

/// At the default size every fact and the prompt heading are visible without
/// scrolling. The prompt excerpt below them may run past the fold.
fn assert_agent_facts_fit_without_scrolling(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    for fact in ["Runtime", "Model", "Reasoning", "Tools", "Source", "PROMPT"] {
        if !screen.contains(fact) {
            anyhow::bail!("agent fact {fact:?} is not visible without scrolling:\n{screen}");
        }
    }
    Ok(())
}

fn assert_narrow_agents_popup(harness: &mut PtyHarness) -> Result<()> {
    let screen = harness.screen().contents();
    if !screen.contains("Loaded agents") {
        anyhow::bail!("narrow agents popup missing title:\n{screen}");
    }
    // Side-by-side layout joins the column divider to the frame with `┬`;
    // stacked layout has none. Counting `│` no longer works because pane
    // scrollbar tracks reuse that glyph.
    if screen.contains('┬') {
        anyhow::bail!("narrow agents popup still used a side-by-side separator:\n{screen}");
    }
    Ok(())
}

/// The stacked narrow layout gives detail fewer rows, so the prompt heading
/// starts below the fold; End must bring it into view and hide the title.
fn assert_narrow_detail_scrolled_to_end(harness: &mut PtyHarness) -> Result<()> {
    assert_narrow_agents_popup(harness)?;
    let screen = harness.screen().contents();
    if !screen.contains("PROMPT") || screen.contains("Internal agent that evaluates") {
        anyhow::bail!("narrow detail did not scroll to its end:\n{screen}");
    }
    Ok(())
}

pub(super) const OPEN_AGENTS_PICKER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    Step::SubmitText("/agents"),
    Step::WaitText {
        text: "INTERNAL",
        timeout: SETTLE,
    },
    Step::TypeText("goal-judge"),
    Step::WaitText {
        text: "Internal agent that evaluates goal completion",
        timeout: SETTLE,
    },
    Step::AssertText("↑↓"),
    Step::AssertText("PgUp/PgDn"),
    Step::Custom(assert_wide_popup_divider_is_stable),
    Step::Custom(assert_agent_facts_fit_without_scrolling),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "Use conversation model",
        timeout: SETTLE,
    },
    Step::AssertText("select model for goal-judge"),
    Step::Key(Key::Esc),
    Step::WaitText {
        text: "Loaded agents",
        timeout: SETTLE,
    },
    Step::Phase("narrow_layout"),
    Step::Resize { rows: 24, cols: 50 },
    Step::WaitQuiet {
        quiet_for: Duration::from_millis(150),
        timeout: SETTLE,
    },
    Step::Custom(assert_narrow_agents_popup),
    // Scroll keys page the navigation list until the detail pane takes focus.
    Step::Key(Key::Right),
    Step::Key(Key::End),
    Step::WaitText {
        text: "PROMPT",
        timeout: SETTLE,
    },
    Step::Custom(assert_narrow_detail_scrolled_to_end),
    // Home returns the focused detail pane to the title.
    Step::Key(Key::Home),
    Step::WaitText {
        text: "Internal agent that evaluates goal",
        timeout: SETTLE,
    },
    Step::Key(Key::Esc),
    Step::ExitCommand,
];
