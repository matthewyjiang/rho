//! Host-specific plan authority: catalog, config, tools, and executable freeze.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    agent::{AgentOrigin, PromptPolicy, ToolCapability, BUILTIN_TOOL_CAPABILITIES},
    app::agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundRuntime},
    app::sdk_config,
    workflow::{
        freeze_directory_identity, freeze_executable_identity, ExecutableIdentity, NodeExecution,
        ResolvedAgent, ResolvedCommand, ResolvedNode,
    },
};

/// Resolves agents and executables while freezing plan authority.
///
/// CLI hosts discover and freeze from the trusted local filesystem. Tool hosts
/// supply identities already authorized through `ToolContext`.
pub(crate) trait PlanHost: Send + Sync {
    fn workspace(&self) -> &Path;
    fn config(&self) -> &crate::config::Config;
    fn catalog(&self) -> &crate::agent::AgentCatalog;
    fn available_tools(&self) -> &crate::agent::AgentCapabilities;

    fn resolve_executable(&self, executable: &str)
        -> anyhow::Result<(PathBuf, ExecutableIdentity)>;
}

/// CLI / trusted-local host: discover executables and freeze on demand.
pub(crate) struct DiscoveringPlanHost<'a> {
    workspace: &'a Path,
    config: &'a crate::config::Config,
    catalog: crate::agent::AgentCatalog,
    available_tools: &'a crate::agent::AgentCapabilities,
}

impl<'a> DiscoveringPlanHost<'a> {
    pub(crate) fn new(
        workspace: &'a Path,
        config: &'a crate::config::Config,
        available_tools: &'a crate::agent::AgentCapabilities,
        workflow_entry: &Path,
    ) -> anyhow::Result<Self> {
        let home = crate::paths::home_dir();
        let trust = if std::env::var_os("RHO_TRUST_PROJECT_AGENTS").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            crate::agent::ProjectTrust::Trusted
        } else {
            crate::agent::ProjectTrust::Untrusted
        };
        Ok(Self {
            workspace,
            config,
            catalog: crate::agent::AgentCatalog::discover_for_workflow_entry(
                workspace,
                workflow_entry,
                home.as_deref(),
                trust,
            )?,
            available_tools,
        })
    }
}

impl PlanHost for DiscoveringPlanHost<'_> {
    fn workspace(&self) -> &Path {
        self.workspace
    }

    fn config(&self) -> &crate::config::Config {
        self.config
    }

    fn catalog(&self) -> &crate::agent::AgentCatalog {
        &self.catalog
    }

    fn available_tools(&self) -> &crate::agent::AgentCapabilities {
        self.available_tools
    }

    fn resolve_executable(
        &self,
        executable: &str,
    ) -> anyhow::Result<(PathBuf, ExecutableIdentity)> {
        let path = resolve_executable_path(executable, self.workspace)?;
        let identity = freeze_executable_identity(&path)?;
        Ok((path, identity))
    }
}

/// Tool host: resolve only from identities already authorized via capabilities.
pub(crate) struct AuthorizedPlanHost<'a> {
    workspace: &'a Path,
    config: &'a crate::config::Config,
    catalog: &'a crate::agent::AgentCatalog,
    available_tools: &'a crate::agent::AgentCapabilities,
    executables: &'a BTreeMap<String, ExecutableIdentity>,
}

impl<'a> AuthorizedPlanHost<'a> {
    pub(crate) fn new(
        workspace: &'a Path,
        config: &'a crate::config::Config,
        catalog: &'a crate::agent::AgentCatalog,
        available_tools: &'a crate::agent::AgentCapabilities,
        executables: &'a BTreeMap<String, ExecutableIdentity>,
    ) -> Self {
        Self {
            workspace,
            config,
            catalog,
            available_tools,
            executables,
        }
    }
}

impl PlanHost for AuthorizedPlanHost<'_> {
    fn workspace(&self) -> &Path {
        self.workspace
    }

    fn config(&self) -> &crate::config::Config {
        self.config
    }

    fn catalog(&self) -> &crate::agent::AgentCatalog {
        self.catalog
    }

    fn available_tools(&self) -> &crate::agent::AgentCapabilities {
        self.available_tools
    }

    fn resolve_executable(
        &self,
        executable: &str,
    ) -> anyhow::Result<(PathBuf, ExecutableIdentity)> {
        let identity = self.executables.get(executable).ok_or_else(|| {
            anyhow::anyhow!("authorized executable identity is missing for '{executable}'")
        })?;
        Ok((
            PathBuf::from(&identity.file.canonical_path),
            identity.clone(),
        ))
    }
}

pub(crate) fn resolve_nodes_with_host(
    graph: &crate::workflow::WorkflowGraph,
    host: &dyn PlanHost,
) -> anyhow::Result<BTreeMap<crate::workflow::NodeId, ResolvedNode>> {
    graph
        .nodes
        .iter()
        .map(|(id, node)| {
            let resolved = match &node.execution {
                NodeExecution::Agent(agent) => {
                    let entry = host.catalog().find(&agent.agent)?;
                    let bound = AgentBinder::bind(
                        Arc::new(entry.definition.clone()),
                        AgentInvocation {
                            role: AgentRole::Workflow,
                            available_tools: host.available_tools().clone(),
                        },
                        host.config(),
                    )?;
                    ResolvedNode::Agent(Box::new(resolve_agent(entry, bound, host)?))
                }
                NodeExecution::Command(command) => {
                    let (executable, cwd) = match command {
                        crate::workflow::CommandNode::Direct {
                            executable, cwd, ..
                        }
                        | crate::workflow::CommandNode::Shell {
                            executable, cwd, ..
                        } => (executable, cwd),
                    };
                    let workspace = host.workspace().canonicalize()?;
                    let cwd_path = workspace.join(cwd).canonicalize()?;
                    if !cwd_path.starts_with(&workspace) {
                        anyhow::bail!("command node '{id}' cwd is outside the workspace");
                    }
                    let (executable_path, executable_identity) =
                        host.resolve_executable(executable)?;
                    ResolvedNode::Command(Box::new(ResolvedCommand {
                        executable_identity,
                        executable: crate::paths::display(&executable_path),
                        exact_path: true,
                        cwd: crate::paths::display(&cwd_path),
                        cwd_identity: freeze_directory_identity(&cwd_path)?,
                        environment_policy: "inherit-current-process".to_owned(),
                    }))
                }
            };
            Ok((id.clone(), resolved))
        })
        .collect()
}

fn resolve_agent(
    entry: &crate::agent::AgentCatalogEntry,
    bound: crate::app::agent_binding::BoundAgent,
    host: &dyn PlanHost,
) -> anyhow::Result<ResolvedAgent> {
    let source_origin = match entry.metadata.origin {
        AgentOrigin::Internal => "internal",
        AgentOrigin::BuiltIn => "built_in",
        AgentOrigin::AgentsHome => "agents_home",
        AgentOrigin::RhoHome => "rho_home",
        AgentOrigin::Project => "project",
        AgentOrigin::Workflow => "workflow",
    };
    let source_origin = match &entry.metadata.path {
        Some(path) => format!("{source_origin}:{}", crate::paths::display(path)),
        None => source_origin.to_owned(),
    };
    let prompt_policy = match &entry.definition.prompt {
        PromptPolicy::Extend(text) => format!("extend:{text}"),
        PromptPolicy::Replace(text) => format!("replace:{text}"),
    };
    let permission_ceiling = match bound.runtime() {
        BoundRuntime::Rho { config, .. } => config.permission_mode.to_string(),
        BoundRuntime::ClaudeCli {
            permission_mode, ..
        } => permission_mode.to_string(),
    };
    let common = ResolvedAgent {
        agent_id: entry.definition.id.to_string(),
        fingerprint: entry.fingerprint.to_string(),
        runtime: match bound.runtime() {
            BoundRuntime::Rho { .. } => crate::workflow::AgentRuntime::Rho,
            BoundRuntime::ClaudeCli { .. } => crate::workflow::AgentRuntime::ClaudeCli,
        },
        source_origin,
        // Workflow-local agents ship with the workflow source the user planned.
        // Project catalog agents still require project trust.
        trust_required: entry.metadata.origin == AgentOrigin::Project,
        prompt_policy,
        provider: None,
        model: None,
        reasoning: None,
        step_limit: sdk_config::run_step_limit().get() as u64,
        capabilities: BTreeSet::new(),
        permission_ceiling,
        auth_profile: None,
        executable: None,
        executable_identity: None,
        arguments: Vec::new(),
    };
    Ok(match bound.runtime() {
        BoundRuntime::Rho {
            config,
            capabilities,
        } => ResolvedAgent {
            provider: Some(config.provider.clone()),
            model: Some(config.model.clone()),
            reasoning: Some(config.reasoning.to_string()),
            capabilities: frozen_capabilities(capabilities),
            auth_profile: Some(config.auth.clone()),
            ..common
        },
        BoundRuntime::ClaudeCli {
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            effort,
        } => {
            let (executable, executable_identity) = host.resolve_executable("claude")?;
            let plan = crate::claude_runtime::spawn::build_spawn_plan(
                &crate::claude_runtime::spawn::ClaudeSpawnRequest {
                    system_prompt: entry.definition.prompt.clone(),
                    model: model.clone(),
                    tools: tools.clone(),
                    inherit_claude_config: *inherit_claude_config,
                    permission_mode: *permission_mode,
                    cwd: host.workspace().to_path_buf(),
                    max_turns: *max_turns,
                    effort: *effort,
                },
            )?;
            ResolvedAgent {
                model: model.clone(),
                reasoning: effort.map(str::to_owned),
                step_limit: *max_turns,
                capabilities: tools.iter().cloned().collect(),
                executable: Some(crate::paths::display(&executable)),
                executable_identity: Some(executable_identity),
                arguments: plan.args,
                ..common
            }
        }
    })
}

fn frozen_capabilities(capabilities: &crate::agent::AgentCapabilities) -> BTreeSet<String> {
    BUILTIN_TOOL_CAPABILITIES
        .iter()
        .filter(|capability| capabilities.contains(capability))
        .filter(|capability| {
            !matches!(
                capability,
                ToolCapability::Agent
                    | ToolCapability::Agents
                    | ToolCapability::Questionnaire
                    | ToolCapability::Rho
                    | ToolCapability::Workflow
            )
        })
        .map(|capability| capability.as_str().to_owned())
        .collect()
}

fn resolve_executable_path(executable: &str, workspace: &Path) -> anyhow::Result<PathBuf> {
    let path = Path::new(executable);
    let resolved = if path.components().count() == 1 {
        crate::executable::find_on_path(executable)
            .ok_or_else(|| anyhow::anyhow!("executable '{executable}' was not found on PATH"))?
    } else if path.is_absolute() {
        path.canonicalize()?
    } else {
        workspace.join(path).canonicalize()?
    };
    Ok(resolved)
}
