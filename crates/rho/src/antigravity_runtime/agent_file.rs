//! The run's agy custom agent, which carries the tool allowlist agy enforces.
//!
//! agy 1.5.0 discovers custom agents only under `.agents/agents/` in a
//! workspace root, `~/.gemini/config/agents/`, or a plugin; it ignores a path
//! passed to `--agent`. So each run writes its own workspace root under the
//! run directory and passes `--add-dir <root> --agent rho-run`, touching
//! neither the user's project nor their agy config. The added root is visible
//! to the agent but holds only this file.
//!
//! agy silently falls back to its default agent (every built-in) when the
//! named agent is not found. A failed write fails the run before spawn, and
//! the fence still refuses undeclared commands (see `fence`).

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::agent::AntigravityTool;

/// Agent name passed to `--agent`.
pub(crate) const AGENT_NAME: &str = "rho-run";

/// Write the agent under `run_dir` and return the workspace root to pass to
/// `--add-dir`.
pub(crate) fn write(run_dir: &Path, tools: &[AntigravityTool]) -> io::Result<PathBuf> {
    let root = run_dir.join("agy");
    let agents = root.join(".agents").join("agents");
    std::fs::create_dir_all(&agents)?;
    std::fs::write(agents.join(format!("{AGENT_NAME}.md")), render(tools))?;
    Ok(root)
}

/// The agent Markdown. The body is empty: Rho's prompt policy travels in the
/// first prompt, and agy keeps its default system prompt.
fn render(tools: &[AntigravityTool]) -> String {
    let mut out = format!("---\nname: {AGENT_NAME}\ndescription: Rho delegated run.\ntools:\n");
    for tool in tools {
        out.push_str("  - ");
        out.push_str(tool.as_name());
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

#[cfg(test)]
#[path = "agent_file_tests.rs"]
mod tests;
