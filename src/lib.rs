mod agent;
mod app;
mod auth;
mod cli;
mod clipboard_image;
mod commands;
mod config;
mod config_writer;
mod credentials;
#[cfg(test)]
#[path = "fixes_validation_tests.rs"]
mod fixes_validation_tests;
mod herdr;
mod keybindings;
mod model;
mod paths;
mod prompt;
mod prompt_templates;
mod provider;
mod provider_backend;
mod reasoning;
mod session;
mod skills;
mod tool;
mod tools;
mod transcript;
mod tui;
mod update;
mod workspace;

pub use app::run;
pub use cli::Cli;
