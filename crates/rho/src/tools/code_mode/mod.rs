//! Starlark `codemode` prototype: compose ToolHost tools (native + MCP) from a script.
//!
//! See `docs/design/code-mode-starlark-v0.md`.

mod bridge;
mod engine;
mod exposure;
mod nesting;
mod search;
mod tool;
mod tool_search;

#[cfg(test)]
#[path = "code_mode_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "live_wiring_tests.rs"]
mod live_wiring_tests;

#[allow(unused_imports)]
pub use bridge::{BridgeError, CodeModeBridge, GuardedBridge, ToolHostBridge, CODEMODE_TOOL_NAME};
#[allow(unused_imports)]
pub use engine::{
    evaluate_code_mode_with_exposure, format_engine_output, EngineError, EngineLimits, EngineOutput,
};
#[allow(unused_imports)]
pub use exposure::{
    format_mcp_servers_catalog, is_mcp_tool_name, ExposureController, ExposureOverride,
    ExposurePolicy, ToolCatalogEntry, ToolExposure,
};
pub use nesting::CodeModeNesting;
#[allow(unused_imports)]
pub use search::{ToolSearchTool, TOOL_SEARCH_NAME};
#[allow(unused_imports)]
pub use tool::CodeModeTool;
