// `rho acp` is a client of the rho daemon, which is unix-only (#1665).
#[cfg(unix)]
mod acp;
#[cfg(debug_assertions)]
mod acp_fixture_agent;
mod active_prompt;
mod agent_binding;
pub(crate) mod agent_concurrency;
pub(crate) mod agent_executor;
mod attach;
mod automation;
pub(crate) mod automation_protocol;
mod bootstrap;
pub(crate) mod boundary_results;
mod classifier_eval;
mod cli_config;
mod compaction_eval;
pub(crate) mod config_repository;
pub(crate) mod context_report;
pub(crate) mod conversation_switch;
#[cfg(unix)]
mod credential_writes;
mod daemon_cli;
// The rho daemon is unix-only (#1665). The TUI follows daemon sessions
// through it too (`tui::daemon_session`, #1667).
#[cfg(unix)]
pub(crate) mod daemon_client;
#[cfg(unix)]
mod daemon_host;
mod detach;
mod doctor_cli;
#[cfg(unix)]
mod doctor_daemon;
pub(crate) mod endpoint_login;
mod headless_host;
mod host_mcp;
mod interactive;
pub(crate) mod interactive_presenter;
mod interactive_run_controller;
pub(crate) mod interactive_runtime;
mod interactive_session_controller;
mod interactive_state;
mod login;
mod mcp_cli;
// `rho mcp serve` is a client of the rho daemon, which is unix-only (#1665).
#[cfg(unix)]
mod mcp_server;
mod model_compactor;
mod model_prompt_cli;
mod model_prompt_metadata;
pub(crate) mod notification_delivery;
mod parent_bridge;
mod parent_steering;
mod plugins_cli;
mod policy;
mod prompt_content;
mod provider_controller;
mod runtime_builder;
pub(crate) mod sdk_config;
mod session_assembly;
mod session_commands;
// Session control through the rho daemon, which is unix-only (#1665).
#[cfg(unix)]
pub(crate) mod session_control;
mod session_events;
mod session_mcp;
mod session_options;
mod session_runtime;
mod sessions_cli;
mod shutdown_signal;
pub(crate) mod side_chat;
pub(crate) mod subagent_host_input;
pub(crate) mod subagent_manager;
pub(crate) mod subagent_messaging;
pub(crate) mod subagent_pool;
mod tools_prompt;
pub(crate) mod workflow_cli;
pub(crate) mod workflow_runtime;

pub(crate) use agent_binding::relaunch_config;
pub use automation::{AutomationExit, AutomationInterrupted};
pub use bootstrap::run;
pub(crate) use session_mcp::session_mcp_inventory;
pub(crate) use session_options::reasoning_from_wire;
pub(crate) use subagent_manager::SubagentManager;
