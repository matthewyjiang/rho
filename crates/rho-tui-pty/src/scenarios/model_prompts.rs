//! Model switches must activate matching instructions or leave the session usable.

use anyhow::{Context, Result};

use super::{
    assert_helpers::wait_for_turn_completion_after,
    pickers::{setup_pinned_models, OPENAI_AND_XAI_KEY_ENV},
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    env::IsolatedHome,
    keys::Key,
    scenario::{Scenario, Step},
    PtyHarness,
};

pub(super) const MODEL_PROMPTS_SCENARIO: Scenario = Scenario::new(
    "model_prompt_switch",
    "Switch model prompts and recover from an invalid prompt file",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("model prompt seed"),
        Step::WaitText {
            text: "fixture response: model prompt seed",
            timeout: STREAM,
        },
        Step::Phase("replacement_prompt"),
        Step::SubmitText("/model xai/grok-4.6"),
        Step::WaitText {
            text: "replacement.md",
            timeout: SETTLE,
        },
        Step::SubmitText("model prompt replaced"),
        Step::WaitText {
            text: "fixture response: model prompt replaced",
            timeout: STREAM,
        },
        Step::Phase("append_prompt"),
        Step::SubmitText("/model openai/gpt-5.5"),
        Step::WaitText {
            text: "appended.md",
            timeout: SETTLE,
        },
        Step::Phase("invalid_prompt_keeps_current_model"),
        Step::Custom(invalidate_replacement),
        Step::SubmitText("/model xai/grok-4.6"),
        Step::WaitText {
            text: "could not switch to xai/grok-4.6",
            timeout: SETTLE,
        },
        Step::SubmitText("model prompt still usable"),
        Step::WaitText {
            text: "fixture response: model prompt still usable",
            timeout: STREAM,
        },
        // A new session builds its model prompt from disk as its first
        // prompt opens it; the failure must hand the prompt back instead of
        // exiting Rho, and the prompt goes once the file is fixed.
        Step::Phase("failed_open_keeps_the_prompt"),
        Step::Custom(wait_for_usable_turn_completion),
        Step::SubmitText("/new"),
        Step::WaitText {
            text: "new session",
            timeout: SETTLE,
        },
        Step::SubmitText("model prompt after failed open"),
        Step::WaitText {
            text: "could not open the session",
            timeout: STREAM,
        },
        Step::AssertText("model prompt after failed open"),
        Step::Custom(restore_replacement),
        Step::Key(Key::Enter),
        Step::WaitText {
            text: "fixture response: model prompt after failed open",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ true,
)
.with_setup(setup)
.with_env(OPENAI_AND_XAI_KEY_ENV);

/// A model chosen before the session's first prompt names its prompt file
/// once that prompt opens the session, as a switch later in it does.
pub(super) const BEFORE_FIRST_PROMPT_SCENARIO: Scenario = Scenario::new(
    "model_prompt_before_first_prompt",
    "Name the prompt file of a model chosen before the first prompt",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/model xai/grok-4.6"),
        Step::WaitText {
            text: "model switched to xai/grok-4.6",
            timeout: SETTLE,
        },
        // The session opens on it with the first prompt, and says so then.
        Step::SubmitText("hello"),
        Step::WaitText {
            text: "replacement.md",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup)
.with_env(OPENAI_AND_XAI_KEY_ENV);

fn setup(home: &IsolatedHome) -> Result<()> {
    setup_pinned_models(home)?;
    let directory = home.home.join(".rho/model-prompts");
    std::fs::create_dir_all(&directory)?;
    for (file, provider, model, mode) in [
        ("appended.md", "openai", "gpt-5.5", "append"),
        ("replacement.md", "xai", "grok-4.6", "replace"),
    ] {
        std::fs::write(directory.join(file), format!(
            "---\nprovider: {provider}\nmodel: {model}\nmode: {mode}\n---\nUse the fixture tools when requested.\n",
        ))?;
    }
    Ok(())
}

fn wait_for_usable_turn_completion(harness: &mut PtyHarness) -> Result<()> {
    wait_for_turn_completion_after(harness, "fixture response: model prompt still usable")
}

fn replacement_path(harness: &PtyHarness) -> Result<std::path::PathBuf> {
    let root = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?;
    Ok(root.join("home/.rho/model-prompts/replacement.md"))
}

fn restore_replacement(harness: &mut PtyHarness) -> Result<()> {
    std::fs::write(
        replacement_path(harness)?,
        "---\nprovider: xai\nmodel: grok-4.6\nmode: replace\n---\nUse the fixture tools when requested.\n",
    )?;
    Ok(())
}

fn invalidate_replacement(harness: &mut PtyHarness) -> Result<()> {
    std::fs::write(replacement_path(harness)?, "invalid frontmatter")?;
    Ok(())
}
