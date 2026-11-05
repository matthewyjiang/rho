use super::*;
use pretty_assertions::assert_eq;

fn tools(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// Everything a fence decides, for whole-value comparison.
fn shape(fence: &ClaudeFence) -> (&'static str, &'static [&'static str]) {
    (fence.session_mode(), fence.setting_sources())
}

// Covers: each Rho mode picks its Claude mode and settings; Auto / Allow edits run headless only when every tool is proven free
// for that Rho class. Specifiers, inherited Claude settings, write or process
// tools, and unknown / plugin / MCP names fail closed; Supervised refuses.
// Owner: Claude fence (permission boundary of a delegated run).
#[test]
fn rho_permission_modes_map_to_the_claude_fence() {
    let headless = || Ok(("default", &[][..]));
    let cases: Vec<(
        PermissionMode,
        Vec<String>,
        bool,
        Result<_, ClaudeFenceError>,
    )> = vec![
        (
            PermissionMode::Plan,
            tools(&["Read", "Glob"]),
            false,
            Ok(("plan", &["project"][..])),
        ),
        (
            PermissionMode::Plan,
            tools(&["mcp__server__tool"]),
            true,
            Ok(("plan", &["user", "project", "local"][..])),
        ),
        (
            PermissionMode::Bypass,
            tools(&["Read", "Bash(git status:*)"]),
            true,
            Ok(("bypassPermissions", &["user", "project", "local"][..])),
        ),
        (
            PermissionMode::Bypass,
            tools(&["FutureClaudeTool"]),
            false,
            Ok(("bypassPermissions", &["project"][..])),
        ),
        (
            PermissionMode::Auto,
            tools(&["Read", "Glob"]),
            false,
            headless(),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["Read", "Glob"]),
            false,
            headless(),
        ),
        (
            PermissionMode::Auto,
            tools(&["WebSearch", "WebFetch"]),
            false,
            headless(),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["WebSearch", "WebFetch"]),
            false,
            headless(),
        ),
        (PermissionMode::Auto, tools(&[]), false, headless()),
        (
            PermissionMode::Auto,
            tools(&["Read", "Task"]),
            false,
            headless(),
        ),
        (
            PermissionMode::Auto,
            tools(&["Bash"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::Auto,
            tools(&["Edit"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["Write"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::Auto,
            tools(&["NotebookEdit"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["PowerShell"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::Auto,
            tools(&["Read", "mcp__server__tool"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::Auto,
            tools(&["ListMcpResourcesTool"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["ReadMcpResourceTool"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["FutureClaudeTool"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::Auto,
            tools(&["Read", "Bash(git status:*)"]),
            false,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::AllowEdits,
            tools(&["Read", "Glob"]),
            true,
            Err(ClaudeFenceError::Unbound),
        ),
        (
            PermissionMode::Supervised,
            tools(&["Read"]),
            false,
            Err(ClaudeFenceError::SupervisedUnsupported),
        ),
    ];
    for (mode, tools, inherit, expected) in cases {
        assert_eq!(
            ClaudeFence::new(mode, &tools, inherit).map(|fence| shape(&fence)),
            expected,
            "{mode:?} {tools:?} inherit={inherit}"
        );
    }
}

// Covers: the built-in allowlist lists base names once and never nested
// agents; `allowedTools` keeps every declared specifier except nested agents.
// Owner: Claude fence tool lists.
#[test]
fn tool_lists_keep_specifiers_out_of_the_allowlist_and_drop_nested_agents() {
    let declared = tools(&["Read", "Bash(git *)", "Bash(ls)", "Task", "agent", "Edit"]);
    assert_eq!(
        (tool_base_names(&declared), allowed_tool_entries(&declared)),
        (
            tools(&["Read", "Bash", "Edit"]),
            tools(&["Read", "Bash(git *)", "Bash(ls)", "Edit"])
        )
    );
}
