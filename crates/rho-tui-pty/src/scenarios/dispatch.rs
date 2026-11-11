//! Dispatch scenarios that need a custom multi-session runner.

use anyhow::Result;

use crate::scenario::{ScenarioOutcome, ScenarioRunner};

use super::{
    all_scenarios, calibrated_context, config, continue_latest, resume_scrollback, send_confirm,
    workflow,
};

pub fn run_named(runner: &ScenarioRunner, name: &str) -> Result<ScenarioOutcome> {
    let scenario = all_scenarios()
        .iter()
        .find(|scenario| scenario.id == name)
        .ok_or_else(|| anyhow::anyhow!("unknown scenario '{name}'"))?;
    if name == calibrated_context::SCENARIO.id {
        return calibrated_context::run(runner);
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
    if continue_latest::is_continue_latest_scenario(name) {
        return continue_latest::run_continue_latest(runner);
    }
    #[cfg(unix)]
    if super::daemon_resume::is_daemon_resume_scenario(name) {
        return super::daemon_resume::run_daemon_resume(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_login::SCENARIO.id {
        return super::daemon_login::run(runner);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if name == super::daemon_workflow_hold::SCENARIO.id {
        return super::daemon_workflow_hold::run(runner);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if name == super::daemon_workflow::SCENARIO.id {
        return super::daemon_workflow::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_rewind::SCENARIO.id {
        return super::daemon_rewind::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_tree::SCENARIO.id {
        return super::daemon_tree::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_reports::SCENARIO.id {
        return super::daemon_reports::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_context::SCENARIO.id {
        return super::daemon_context::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_switch::SCENARIO.id {
        return super::daemon_switch::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_detach::SCENARIO.id {
        return super::daemon_detach::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_restart_mcp::SCENARIO.id {
        return super::daemon_restart_mcp::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_restart::SCENARIO.id {
        return super::daemon_restart::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_goal::SCENARIO.id {
        return super::daemon_goal::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_mcp::SCENARIO.id {
        return super::daemon_mcp::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_mcp_owner::SCENARIO.id {
        return super::daemon_mcp_owner::run(runner);
    }
    #[cfg(unix)]
    if name == super::daemon_doctor::SCENARIO.id {
        return super::daemon_doctor::run(runner);
    }
    runner.run(scenario)
}
