//! Claude's per-run fence: which built-ins exist, which settings Claude loads,
//! and the Claude session mode. Rho rejects every permission request Claude
//! sends (see `policy`).
//!
//! All derive from the Rho permission mode, the declared `tools:`, and
//! `inherit_claude_config`, and travel as `_meta.claudeCode.options` on
//! `session/new` plus `session/set_mode` (see `policy`):
//!
//! | Rho mode | Claude mode | settings loaded | permission answers |
//! |---|---|---|---|
//! | Plan | `plan` | inherited or project | reject |
//! | Bypass | `bypassPermissions` | inherited or project | reject (bypass-immune asks only) |
//! | Auto, Allow edits | `default` | none | reject |
//! | Supervised | refused | | |
//!
//! Auto and Allow edits reproduce Claude `dontAsk`: Claude runs read-only
//! tools and `allowedTools` entries without asking and asks Rho for anything
//! else, which Rho rejects. Claude also auto-approves read-only Bash, so those
//! modes refuse unless every declared tool is proven to stay inside the Rho
//! approval class ([`ClaudeFence::new`]).

use std::sync::LazyLock;

use rho_providers::{model::ReasoningLevelSet, reasoning::ReasoningLevel};
use rho_sdk::{CapabilityKind, PolicyDecision};

use crate::permission::PermissionMode;

/// Nested Claude agents stay off: Rho owns delegation.
pub(crate) const DISALLOWED_NESTED_AGENT_TOOLS: [&str; 2] = ["Task", "Agent"];

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClaudeFenceError {
    #[error(
        "claude-cli agents cannot run in Auto or Allow edits unless the child stays \
on its declared tools: Claude auto-approves read-only Bash and allowedTools \
entries. Rho therefore refuses when inherit_claude_config is true, when a tool \
uses a specifier (for example Bash(git *)), or when any declared tool is not \
proven to stay inside that Rho approval class. Unknown Claude, plugin, and MCP \
names fail closed. Switch to Plan or Bypass, use only proven no-prompt tools, \
or disable inherited Claude config."
    )]
    Unbound,
    #[error(
        "claude-cli agents cannot run in Supervised permission mode: a delegated \
Claude run cannot ask you for approval. Switch to Auto, Allow edits, Plan, or \
Bypass, or change the agent. Auto and Allow edits only work when every tool is \
proven not to bypass that Rho approval class and inherit_claude_config is false."
    )]
    SupervisedUnsupported,
}

/// claude-agent-acp session modes Rho sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaudeMode {
    Plan,
    BypassPermissions,
    /// Asks before non-read-only tools; Rho rejects every ask.
    Default,
}

/// Which Claude settings files the session loads (SDK `settingSources`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingSources {
    /// User, project, and local settings: `inherit_claude_config: true`.
    Inherited,
    /// Project settings only.
    Project,
    /// Nothing, so no hook or permission rule can widen the fence.
    None,
}

/// The fence for one run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeFence {
    mode: ClaudeMode,
    settings: SettingSources,
}

impl ClaudeFence {
    pub(crate) fn new(
        mode: PermissionMode,
        tools: &[String],
        inherit_claude_config: bool,
    ) -> Result<Self, ClaudeFenceError> {
        let configured = if inherit_claude_config {
            SettingSources::Inherited
        } else {
            SettingSources::Project
        };
        match mode {
            PermissionMode::Plan => Ok(Self {
                mode: ClaudeMode::Plan,
                settings: configured,
            }),
            PermissionMode::Bypass => Ok(Self {
                mode: ClaudeMode::BypassPermissions,
                settings: configured,
            }),
            PermissionMode::Auto | PermissionMode::AllowEdits => {
                if preserves_bound_set(mode, tools, inherit_claude_config) {
                    // Claude's `dontAsk`: its own auto-approvals run, every
                    // ask is refused, and no settings can widen that.
                    Ok(Self {
                        mode: ClaudeMode::Default,
                        settings: SettingSources::None,
                    })
                } else {
                    Err(ClaudeFenceError::Unbound)
                }
            }
            PermissionMode::Supervised => Err(ClaudeFenceError::SupervisedUnsupported),
        }
    }

    /// Rho's own no-tools calls (advisor): nothing runs without Rho's say,
    /// and no settings load. Plan mode would inject plan-tool prose.
    pub(crate) fn headless() -> Self {
        Self {
            mode: ClaudeMode::Default,
            settings: SettingSources::None,
        }
    }

    pub(crate) fn session_mode(&self) -> &'static str {
        match self.mode {
            ClaudeMode::Plan => "plan",
            ClaudeMode::BypassPermissions => "bypassPermissions",
            ClaudeMode::Default => "default",
        }
    }

    pub(crate) fn setting_sources(&self) -> &'static [&'static str] {
        match self.settings {
            SettingSources::Inherited => &["user", "project", "local"],
            SettingSources::Project => &["project"],
            SettingSources::None => &[],
        }
    }
}

/// Headless runs auto-approve read-only Bash and `allowedTools` entries, so
/// the child stays on the bound set only when Claude settings (and their
/// hooks) stay unloaded and every declared tool is proven not to bypass this
/// mode's approval class.
fn preserves_bound_set(
    mode: PermissionMode,
    tools: &[String],
    inherit_claude_config: bool,
) -> bool {
    !inherit_claude_config
        && tools
            .iter()
            .all(|tool| tool_preserves_bound_set(mode, tool))
}

fn tool_preserves_bound_set(mode: PermissionMode, tool: &str) -> bool {
    let base = tool_base_name(tool);
    if is_nested_agent_tool(base) {
        // Nested agents are always denied separately.
        return true;
    }
    // Specifiers expose a broader base tool through `tools`.
    base == tool && bare_tool_preserves_approval_boundary(mode, base)
}

fn bare_tool_preserves_approval_boundary(mode: PermissionMode, base: &str) -> bool {
    // Fail closed: only known Claude built-ins whose Rho *kind* is freely
    // allowed in this mode qualify. This uses `decision_for`, the kind-only
    // mapping, not `ModePolicy::evaluate`: Claude is a subprocess with its own
    // filesystem view, like bash, so Rho's path-scoped read gate does not
    // apply. Unknown, plugin, MCP, and Claude MCP resource tools have no
    // proven class: a server resource URI can do more than Rho Read.
    let Some(kind) = tool_capability_kind(base) else {
        return false;
    };
    matches!(mode.decision_for(kind), PolicyDecision::Allow)
}

/// The Rho capability class a Claude built-in can exercise. `None` means
/// unproven (new Claude tools, plugins, MCP, and Claude MCP resource tools).
fn tool_capability_kind(base: &str) -> Option<CapabilityKind> {
    Some(match base.to_ascii_lowercase().as_str() {
        "read" | "glob" | "grep" | "lsp" => CapabilityKind::Read,
        "webfetch" | "websearch" => CapabilityKind::Network,
        "edit" | "write" | "notebookedit" => CapabilityKind::Write,
        "bash" | "powershell" | "monitor" | "skill" => CapabilityKind::Process,
        _ => return None,
    })
}

/// Built-in base names for the `tools` allowlist (a specifier such as
/// `Bash(git *)` lists `Bash`), without nested agents.
pub(crate) fn tool_base_names(tools: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for tool in tools {
        let base = tool_base_name(tool);
        if !is_nested_agent_tool(base) && !names.iter().any(|existing| existing == base) {
            names.push(base.to_owned());
        }
    }
    names
}

/// Every declared entry except nested agents, for `allowedTools`: bare names
/// and specifiers run without asking.
pub(crate) fn allowed_tool_entries(tools: &[String]) -> Vec<String> {
    tools
        .iter()
        .filter(|tool| !is_nested_agent_tool(tool_base_name(tool)))
        .cloned()
        .collect()
}

fn is_nested_agent_tool(base: &str) -> bool {
    DISALLOWED_NESTED_AGENT_TOOLS
        .iter()
        .any(|nested| base.eq_ignore_ascii_case(nested))
}

fn tool_base_name(tool: &str) -> &str {
    tool.split_once('(').map_or(tool, |(base, _)| base)
}

/// Claude effort for a Rho `reasoning:` level. Claude accepts `low`,
/// `medium`, `high`, `xhigh`, and `max`; `off` and `minimal` have none.
pub(crate) fn claude_effort(level: ReasoningLevel) -> Option<&'static str> {
    match level {
        ReasoningLevel::Off | ReasoningLevel::Minimal => None,
        ReasoningLevel::Low => Some("low"),
        ReasoningLevel::Medium => Some("medium"),
        ReasoningLevel::High => Some("high"),
        ReasoningLevel::Xhigh => Some("xhigh"),
        ReasoningLevel::Max => Some("max"),
    }
}

/// Keep Claude inherit (`None`) or a level that maps to a Claude effort.
///
/// Parse already rejects `off` / `minimal` on Claude definitions. Bind and
/// frozen rebuild call this so a constructed config cannot slip past.
pub(crate) fn require_claude_reasoning(
    reasoning: Option<ReasoningLevel>,
) -> anyhow::Result<Option<ReasoningLevel>> {
    match reasoning {
        None => Ok(None),
        Some(level) if claude_effort(level).is_some() => Ok(Some(level)),
        Some(level) => anyhow::bail!(
            "reasoning '{level}' is not a Claude Code effort level; \
expected one of: low, medium, high, xhigh, max (omit to inherit Claude's default)"
        ),
    }
}

/// Reasoning levels a Claude run can actually be asked for, derived from
/// [`claude_effort`] so the mapping is stated once.
pub(crate) static CLAUDE_EFFORT_LEVELS: LazyLock<ReasoningLevelSet> = LazyLock::new(|| {
    ReasoningLevelSet::new(
        ReasoningLevel::ALL
            .into_iter()
            .filter(|level| claude_effort(*level).is_some())
            .collect(),
    )
});

#[cfg(test)]
#[path = "fence_tests.rs"]
mod tests;
