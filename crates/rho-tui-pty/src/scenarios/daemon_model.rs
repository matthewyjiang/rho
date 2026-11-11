//! The model-switch scenarios again, with the daemon holding the session.
//!
//! Each reuses its in-process scenario's steps unchanged: what `/model`, the
//! pin cycle, the model picker and the statusline clicks show must not depend
//! on which side runs the session.

use super::{
    model_cycle_reasoning, model_prompts,
    pickers::{setup_pinned_models, CYCLE_AND_PINNED_MODEL_PICKER_STEPS, OPEN_MODEL_PICKER_STEPS},
    status_clicks, DEFAULT_SIZE,
};
use crate::scenario::Scenario;

const OPENAI_ENV: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "fixture-openai-key"),
    ("RHO_TUI_DAEMON", "1"),
];
const COPILOT_ENV: &[(&str, &str)] = &[
    ("GITHUB_COPILOT_TOKEN", "fixture-copilot-token"),
    ("RHO_TUI_DAEMON", "1"),
];
const OPENAI_AND_XAI_ENV: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "fixture-openai-key"),
    ("XAI_API_KEY", "fixture-xai-key"),
    ("RHO_TUI_DAEMON", "1"),
];
const PINNED_ENV: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "fixture-openai-key"),
    ("XAI_API_KEY", "fixture-xai-key"),
    ("POOLSIDE_API_KEY", "fixture-poolside-key"),
    ("RHO_TUI_DAEMON", "1"),
];

pub(super) const OPEN_MODEL_PICKER: Scenario = Scenario::new(
    "daemon_open_model_picker",
    "Open and dismiss a daemon session's model picker",
    DEFAULT_SIZE,
    OPEN_MODEL_PICKER_STEPS,
    /*smoke*/ false,
)
.with_env(OPENAI_ENV);

pub(super) const CYCLE_AND_PINNED_MODEL_PICKER: Scenario = Scenario::new(
    "daemon_cycle_and_pinned_model_picker",
    "Cycle a daemon session's pinned models and toggle the /model pinned view",
    DEFAULT_SIZE,
    CYCLE_AND_PINNED_MODEL_PICKER_STEPS,
    /*smoke*/ false,
)
.with_setup(setup_pinned_models)
.with_env(OPENAI_AND_XAI_ENV);

pub(super) const STATUSLINE_MODEL_CLICK: Scenario = Scenario {
    id: "daemon_statusline_model_click",
    description: "Open a daemon session's model picker by clicking the statusline model",
    env: OPENAI_ENV,
    ..status_clicks::STATUSLINE_MODEL_CLICK_SCENARIO
};

pub(super) const STATUSLINE_NO_REASONING_CLICK: Scenario = Scenario {
    id: "daemon_statusline_no_reasoning_click",
    description: "Paint no reasoning target for a daemon session's model without reasoning",
    env: COPILOT_ENV,
    ..status_clicks::STATUSLINE_NO_REASONING_CLICK_SCENARIO
};

pub(super) const MODEL_PROMPTS: Scenario = Scenario {
    id: "daemon_model_prompt_switch",
    description: "Switch a daemon session's model prompts and recover from an invalid prompt file",
    env: OPENAI_AND_XAI_ENV,
    smoke: false,
    ..model_prompts::MODEL_PROMPTS_SCENARIO
};

pub(super) const CYCLE_REASONING_IDLE: Scenario = Scenario {
    id: "daemon_cycle_reasoning_idle",
    description: "Keep or normalize a daemon session's reasoning across pinned model cycles",
    env: PINNED_ENV,
    smoke: false,
    ..model_cycle_reasoning::IDLE
};

pub(super) const CYCLE_REASONING_QUEUED: Scenario = Scenario {
    id: "daemon_cycle_reasoning_queued",
    description: "Normalize a pinned model switch queued during a daemon turn",
    env: PINNED_ENV,
    smoke: false,
    ..model_cycle_reasoning::QUEUED
};
