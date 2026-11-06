//! A session with one command of every kind, for tests of the shared
//! resolver and of what hosts advertise.

use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::session_runtime::SessionTools,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    tools::{
        mcp::catalog::{McpPrompt, McpPromptArgument},
        sdk_registry::{AppToolSet, ToolSetOptions},
    },
};

use super::CommandScope;

/// A workspace with one prompt template (`review`), one project skill
/// (`demo`), and one workflow (`check`), user definitions for a Rho
/// (`child-rho`) and an external (`child-claude`) agent, an offline MCP prompt
/// (`docs:search`), and a session that can load skills, start workflows,
/// and write files (but not delegate).
pub(in crate::app) struct Fixture {
    workspace: tempfile::TempDir,
    home: tempfile::TempDir,
    tools: AppToolSet,
    config: Config,
}

impl Fixture {
    pub(in crate::app) fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let prompts = workspace.path().join(".rho/prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(prompts.join("review.md"), "Review $1 carefully").unwrap();
        let skill = workspace.path().join(".agents/skills/demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo skill.\n---\nBody\n",
        )
        .unwrap();
        let workflows = workspace.path().join(".rho/workflows/check");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("workflow.star"), "").unwrap();
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".rho/agents");
        std::fs::create_dir_all(&agents).unwrap();
        for (id, contents) in [
            (
                "child-rho",
                "---\ndescription: Rho child\ntools: [read_file]\n---\nInspect files\n",
            ),
            (
                "child-claude",
                "---\ndescription: Claude child\nruntime: claude-cli\ntools: [Read]\n---\nInspect files\n",
            ),
        ] {
            std::fs::write(agents.join(format!("{id}.md")), contents).unwrap();
        }
        let config = Config::default();
        let tools = AppToolSet::new(
            &config,
            RuntimeDiagnostics::new(&config),
            ToolSetOptions::new(AgentCapabilities::new(
                [
                    ToolCapability::Skill,
                    ToolCapability::Workflow,
                    ToolCapability::WriteFile,
                    ToolCapability::Edit,
                ]
                .into(),
            ))
            .workflow(crate::app::workflow_cli::workflow_tool_service(
                workspace.path().to_path_buf(),
                /*config_path*/ None,
                crate::tools::workflow_tracker::WorkflowRunTracker::new(),
            )),
        );
        tools.mcp_catalog().insert_offline_prompt(
            McpPrompt {
                server: "docs".into(),
                name: "search".into(),
                title: None,
                description: Some("Search the docs".into()),
                arguments: vec![McpPromptArgument {
                    name: "query".into(),
                    description: None,
                    required: true,
                }],
            },
            /*completions*/ false,
        );
        Self {
            workspace,
            home,
            tools,
            config,
        }
    }

    pub(in crate::app) fn workspace(&self) -> &std::path::Path {
        self.workspace.path()
    }

    /// The session in supervised mode.
    pub(in crate::app) fn scope(&self) -> CommandScope<'_> {
        CommandScope {
            cwd: self.workspace.path(),
            home: Some(self.home.path()),
            tools: SessionTools::of_tool_set_for_tests(&self.tools),
            config: &self.config,
            permission_mode: PermissionMode::Supervised,
        }
    }
}
