//! Model switches must activate matching instructions or leave the session usable.

use anyhow::{Context, Result};

use super::{
    pickers::{setup_pinned_models, OPENAI_AND_XAI_KEY_ENV},
    DEFAULT_SIZE, SETTLE, STARTUP, STREAM,
};
use crate::{
    env::IsolatedHome,
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
        Step::ExitCommand,
    ],
    /*smoke*/ true,
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

fn invalidate_replacement(harness: &mut PtyHarness) -> Result<()> {
    let root = harness
        .working_directory()
        .and_then(std::path::Path::parent)
        .context("matrix workspace has no isolated home parent")?;
    std::fs::write(
        root.join("home/.rho/model-prompts/replacement.md"),
        "invalid frontmatter",
    )?;
    Ok(())
}
