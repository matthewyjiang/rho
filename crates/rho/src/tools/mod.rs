pub mod agent;
mod agent_output;
mod coding;
mod process;
pub mod rho;
mod sdk_features;
pub mod sdk_registry;
pub mod skill;
#[cfg(debug_assertions)]
pub(crate) mod tui_fixture;
pub mod web;

#[cfg(test)]
#[path = "app_owned_opt_in_tests.rs"]
mod app_owned_opt_in_tests;
