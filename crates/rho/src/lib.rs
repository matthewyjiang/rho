// rustc overflows the default 128-query limit laying out
// `Instrumented<{async fn body of run_inner()}>` at startup.
#![recursion_limit = "256"]

mod agent;
mod app;
mod changelog;
mod child_env;
mod claude_runtime;
mod cli;
mod clipboard;
mod commands;
mod compaction;
mod config;
mod config_writer;
mod credential_store;
mod diagnostics;
mod executable;
mod export;
mod herdr;
mod hooks;
mod keybindings;
mod logging;
mod model_aliases;
mod model_identity;
mod paths;
mod permission;
mod permission_classifier;
mod permission_classifier_handler;
mod plugins;
mod prompt;
mod prompt_history;
mod prompt_templates;
mod questionnaire;
mod run_artifacts;
mod session;
mod skills;
mod sqlite_privacy;
mod stdio;
mod subagent;
mod title;
mod tools;
mod transcript;
mod tui;
mod update;
mod usage;
mod usage_limits;
mod usage_limits_cache;
pub(crate) mod workflow;
mod workspace;

pub use app::{run, AutomationExit, AutomationInterrupted};
pub use cli::Cli;
pub use rho_providers as providers_lib;
pub use rho_sdk as sdk;
pub use rho_tools as tools_lib;
