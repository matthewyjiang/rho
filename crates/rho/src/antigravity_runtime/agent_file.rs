//! The run's agy custom agent, which carries the tool allowlist agy enforces.
//!
//! agy 1.5.0 discovers custom agents only under `.agents/agents/` in a
//! workspace root, `~/.gemini/config/agents/`, or a plugin; it ignores a path
//! passed to `--agent`. So each run writes its own workspace root under the
//! run directory and passes `--add-dir <root> --agent <name>`, touching
//! neither the user's project nor their agy config. The added root is visible
//! to the agent but holds only this file.
//!
//! The project is a workspace root too, so its `.agents/agents/` is searched
//! as well. Each run's name is random, so a project cannot ship an agent that
//! shadows it.
//!
//! agy silently falls back to its default agent (every built-in) when the
//! named agent is not found. A failed write fails the run before spawn, and
//! the fence still refuses undeclared commands (see `fence`).

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::agent::AntigravityTool;

/// One run's agent: the workspace root for `--add-dir` and the name for
/// `--agent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunAgent {
    pub(crate) root: PathBuf,
    pub(crate) name: String,
}

/// Write a freshly named agent under `run_dir`.
pub(crate) fn write(run_dir: &Path, tools: &[AntigravityTool]) -> io::Result<RunAgent> {
    let name = format!("rho-run-{}", uuid::Uuid::new_v4().simple());
    let root = run_dir.join("agy");
    let agents = root.join(".agents").join("agents");
    std::fs::create_dir_all(&agents)?;
    std::fs::write(agents.join(format!("{name}.md")), render(&name, tools))?;
    Ok(RunAgent { root, name })
}

/// The agent Markdown. The body is empty: Rho's prompt policy travels in the
/// first prompt, and agy keeps its default system prompt.
fn render(name: &str, tools: &[AntigravityTool]) -> String {
    let mut out = format!("---\nname: {name}\ndescription: Rho delegated run.\ntools:\n");
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
