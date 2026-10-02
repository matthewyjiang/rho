//! Starlark composition through the SDK ToolHost, with live tool discovery.

mod bridge;
mod call_log;
mod engine;
mod exposure;
pub(crate) mod script_output;
mod tool;
mod tool_search;

pub(crate) use bridge::CODEMODE_TOOL_NAME;
pub(crate) use call_log::{NestedCallRecord, NestedCallStatus};
pub(crate) use exposure::CodeModeSurface;
#[cfg(test)]
pub(crate) use tool_search::TOOL_SEARCH_NAME;

#[cfg(test)]
#[path = "live_wiring_tests.rs"]
mod live_wiring_tests;
