use pretty_assertions::assert_eq;

use super::ensure_program_launchable;
use crate::{
    agent::{AgentCatalog, AgentCatalogSources, AgentSource},
    workflow::{
        test_support, CommandNode, HandleLaunch, Node, NodeExecution, WorkflowError,
        WorkspaceAccess,
    },
};

/// Built-ins plus a workflow agent, `claude`, on the Claude CLI runtime.
fn catalog() -> AgentCatalog {
    AgentCatalog::from_sources(AgentCatalogSources {
        workflow: vec![AgentSource::read(
            "agents/claude.md".into(),
            "---\ndescription: demo\nruntime: claude-cli\ntools: [Read]\n---\n".into(),
        )],
        ..Default::default()
    })
    .unwrap()
}

/// A node running the built-in Rho `reviewer` agent in process.
fn rho_agent(name: &str) -> Node {
    test_support::agent_node(name, &[], WorkspaceAccess::ReadOnly)
}

fn cli_agent(name: &str) -> Node {
    let mut node = rho_agent(name);
    let NodeExecution::Agent(agent) = &mut node.execution else {
        unreachable!("agent_node builds an agent node");
    };
    agent.agent = "claude".into();
    node
}

fn command(name: &str) -> Node {
    Node {
        execution: NodeExecution::Command(CommandNode::Direct {
            executable: "/bin/true".into(),
            arguments: Vec::new(),
            cwd: ".".into(),
            output: None,
        }),
        ..rho_agent(name)
    }
}

// Covers: planning rejects a workflow on a target that cannot launch frozen
// processes from verified handles when any node would launch one (a command
// or an external CLI agent), with the launch's own error, while a workflow
// of in-process Rho agents still plans there (#1736).
// Owner: workflow planning (plan-time launch check before node resolution).
#[test]
fn planning_rejects_process_nodes_only_where_they_cannot_launch() {
    use HandleLaunch::{Supported, Unsupported};
    let catalog = catalog();
    let cases = [
        (
            "rho agents only",
            vec![rho_agent("a"), rho_agent("b")],
            Unsupported,
            true,
        ),
        (
            "a command node",
            vec![rho_agent("a"), command("build")],
            Unsupported,
            false,
        ),
        (
            "a CLI agent node",
            vec![rho_agent("a"), cli_agent("claude")],
            Unsupported,
            false,
        ),
        (
            "process nodes where they launch",
            vec![command("build"), cli_agent("claude")],
            Supported,
            true,
        ),
    ];
    for (name, nodes, launch, plans) in cases {
        let program = test_support::workflow(nodes).program;

        let result = ensure_program_launchable(&program, &catalog, launch);

        let rejected_for_platform = result.as_ref().err().map(|error| {
            matches!(
                error.downcast_ref::<WorkflowError>(),
                Some(WorkflowError::UnsupportedPlatform)
            )
        });
        assert_eq!(
            rejected_for_platform,
            (!plans).then_some(true),
            "{name}: {:?}",
            result.err()
        );
    }
}
