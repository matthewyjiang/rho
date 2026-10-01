//! Starlark `codemode` prototype: compose ToolHost tools (native + MCP) from a script.
//!
//! See `docs/design/code-mode-starlark-v0.md`.

mod bridge;
mod engine;
mod search;
mod tool;

#[cfg(test)]
#[path = "code_mode_tests.rs"]
mod tests;

#[allow(unused_imports)]
pub use bridge::{
    BridgeError, CodeModeBridge, GuardedBridge, ToolHostBridge, CODEMODE_TOOL_NAME,
};
#[allow(unused_imports)]
pub use engine::{evaluate_code_mode, format_engine_output, EngineError, EngineLimits, EngineOutput};
#[allow(unused_imports)]
pub use search::{ToolExposure, ToolSearch, ToolSearchHit, UnimplementedToolSearch};
#[allow(unused_imports)]
pub use tool::CodeModeTool;
