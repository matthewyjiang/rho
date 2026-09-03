//! The Claude Code tool names Rho offers when a surface picks them.
//!
//! Claude accepts any `Tool` or `Tool(specifier)` string on `--tools`, so this
//! list is not a validator: it is the set of rows a picker shows. Names Rho
//! does not offer (MCP tools, specifiers such as `Bash(git *)`) stay valid and
//! keep their own row when a definition already carries them.

/// One offered Claude Code tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeTool {
    /// Exact name passed through on `--tools`.
    pub(crate) name: &'static str,
    /// Row detail text, phrased for a picker.
    pub(crate) detail: &'static str,
}

/// Built-in Claude Code tools a subagent commonly needs, per the Claude Code
/// tools reference. Session-only tools (plan mode, worktrees, feedback, cron)
/// are omitted on purpose.
pub(crate) const CLAUDE_TOOLS: &[ClaudeTool] = &[
    ClaudeTool {
        name: "Read",
        detail: "Read files, images, PDFs, and notebooks.",
    },
    ClaudeTool {
        name: "Edit",
        detail: "Exact string replacement in existing files.",
    },
    ClaudeTool {
        name: "Write",
        detail: "Create or overwrite files.",
    },
    ClaudeTool {
        name: "NotebookEdit",
        detail: "Edit Jupyter notebook cells.",
    },
    ClaudeTool {
        name: "Glob",
        detail: "Find files by pattern.",
    },
    ClaudeTool {
        name: "Grep",
        detail: "Search file contents with ripgrep.",
    },
    ClaudeTool {
        name: "LSP",
        detail: "Language server navigation and diagnostics.",
    },
    ClaudeTool {
        name: "Bash",
        detail: "Run shell commands.",
    },
    ClaudeTool {
        name: "PowerShell",
        detail: "Run PowerShell commands.",
    },
    ClaudeTool {
        name: "Monitor",
        detail: "Watch a command or WebSocket in the background.",
    },
    ClaudeTool {
        name: "WebFetch",
        detail: "Fetch and summarize a URL.",
    },
    ClaudeTool {
        name: "WebSearch",
        detail: "Search the web.",
    },
    ClaudeTool {
        name: "Agent",
        detail: "Spawn a nested subagent.",
    },
    ClaudeTool {
        name: "Skill",
        detail: "Run a skill.",
    },
    ClaudeTool {
        name: "AskUserQuestion",
        detail: "Ask the user multiple-choice questions.",
    },
    ClaudeTool {
        name: "TaskCreate",
        detail: "Create a task in the session task list.",
    },
    ClaudeTool {
        name: "TaskGet",
        detail: "Read one task's details.",
    },
    ClaudeTool {
        name: "TaskList",
        detail: "List session tasks.",
    },
    ClaudeTool {
        name: "TaskUpdate",
        detail: "Update or delete a task.",
    },
    ClaudeTool {
        name: "TodoWrite",
        detail: "Legacy session checklist tool.",
    },
    ClaudeTool {
        name: "ToolSearch",
        detail: "Load deferred tools on demand.",
    },
    ClaudeTool {
        name: "ListMcpResourcesTool",
        detail: "List resources on connected MCP servers.",
    },
    ClaudeTool {
        name: "ReadMcpResourceTool",
        detail: "Read one MCP resource by URI.",
    },
];

/// Whether `name` is one of the offered tools.
///
/// Decides whether a configured tool already has a row, not whether Claude
/// accepts it: specifiers and MCP names are valid and never listed here.
pub(crate) fn is_offered_tool(name: &str) -> bool {
    CLAUDE_TOOLS.iter().any(|tool| tool.name == name)
}
