//! Dispatch scenarios that need a custom multi-session runner.

use anyhow::Result;

use crate::scenario::{ScenarioOutcome, ScenarioRunner};

use super::{all_scenarios, calibrated_context, config, resume_scrollback, send_confirm, workflow};

pub fn run_named(runner: &ScenarioRunner, name: &str) -> Result<ScenarioOutcome> {
    let scenario = all_scenarios()
        .iter()
        .find(|scenario| scenario.id == name)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario '{name}'"))?;
    if name == calibrated_context::SCENARIO.id {
        return calibrated_context::run(runner);
    }
    #[cfg(unix)]
    if name == super::computer_preference::COMPUTER_PREFERENCE_SCENARIO.id {
        return super::computer_preference::run(runner);
    }
    if workflow::is_workflow_scenario(name) {
        return workflow::run(runner, name);
    }
    if config::is_auto_recovered_handoff_scenario(name) {
        return config::run_auto_recovered_handoff(runner);
    }
    if send_confirm::is_send_confirm_scenario(name) {
        return send_confirm::run_send_confirm_handoff(runner);
    }
    if resume_scrollback::is_resume_scrollback_scenario(name) {
        return resume_scrollback::run_resume_scrollback(runner);
    }
    runner.run(scenario)
}
