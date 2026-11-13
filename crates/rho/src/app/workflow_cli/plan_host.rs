//! Host-specific plan authority: catalog, config, tools, and executable freeze.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    agent::{AgentOrigin, PromptPolicy, ToolCapability, BUILTIN_TOOL_CAPABILITIES},
    app::agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundRuntime},
    app::sdk_config,
    workflow::{
        freeze_directory_identity, freeze_executable_identity, ExecutableIdentity, HandleLaunch,
        NodeExecution, ResolvedAgent, ResolvedCommand, ResolvedNode,
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
    /// The credentials of whoever the run will act for, which decide the
    /// models its agent nodes can use.
    fn credentials(&self) -> &dyn rho_providers::credentials::CredentialStore;

    fn resolve_executable(&self, executable: &str)
        -> anyhow::Result<(PathBuf, ExecutableIdentity)>;
}

/// CLI / trusted-local host: discover executables and freeze on demand.
pub(crate) struct DiscoveringPlanHost<'a> {
    workspace: &'a Path,
    config: &'a crate::config::Config,
    catalog: crate::agent::AgentCatalog,
    available_tools: &'a crate::agent::AgentCapabilities,
    /// The `PATH` bare executables and `#!/usr/bin/env` interpreters are
    /// found on: that of the environment the run will start in (#1659).
    search_path: Option<&'a OsStr>,
    credentials: &'a dyn rho_providers::credentials::CredentialStore,
}

impl<'a> DiscoveringPlanHost<'a> {
    /// A host finding bare executables on `search_path` and project agents
    /// under `trust`, and binding agents with `credentials`, all of the
    /// environment the run will start in.
    pub(crate) fn new(
        workspace: &'a Path,
        config: &'a crate::config::Config,
        available_tools: &'a crate::agent::AgentCapabilities,
        workflow_entry: &Path,
        search_path: Option<&'a OsStr>,
        trust: crate::workspace::ProjectTrusts,
        credentials: &'a dyn rho_providers::credentials::CredentialStore,
    ) -> anyhow::Result<Self> {
        let home = crate::paths::home_dir();
        let plugins = crate::plugins::agent_tiers(workspace, home.as_deref(), trust);
        Ok(Self {
            workspace,
            config,
            catalog: crate::agent::AgentCatalog::discover_for_workflow_entry(
                workspace,
                workflow_entry,
                home.as_deref(),
                trust.agents,
                &plugins,
            )?,
            available_tools,
            search_path,
            credentials,
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

    fn credentials(&self) -> &dyn rho_providers::credentials::CredentialStore {
        self.credentials
    }

    fn resolve_executable(
        &self,
        executable: &str,
    ) -> anyhow::Result<(PathBuf, ExecutableIdentity)> {
        let path = resolve_executable_path(executable, self.workspace, self.search_path)?;
        let identity = freeze_executable_identity(&path, self.search_path)?;
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
    credentials: &'a dyn rho_providers::credentials::CredentialStore,
}

impl<'a> AuthorizedPlanHost<'a> {
    pub(crate) fn new(
        workspace: &'a Path,
        config: &'a crate::config::Config,
        catalog: &'a crate::agent::AgentCatalog,
        available_tools: &'a crate::agent::AgentCapabilities,
        executables: &'a BTreeMap<String, ExecutableIdentity>,
        credentials: &'a dyn rho_providers::credentials::CredentialStore,
    ) -> Self {
        Self {
            workspace,
            config,
            catalog,
            available_tools,
            executables,
            credentials,
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

    fn credentials(&self) -> &dyn rho_providers::credentials::CredentialStore {
        self.credentials
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

/// A workflow program whose nodes can all launch on the planning target.
///
/// Node resolution and executable-read authorization take only this, so no
/// planning adapter can skip [`ensure_program_launchable`].
#[derive(Clone, Copy)]
pub(crate) struct LaunchableProgram<'a>(&'a crate::workflow::WorkflowProgram);

impl<'a> LaunchableProgram<'a> {
    pub(crate) fn program(self) -> &'a crate::workflow::WorkflowProgram {
        self.0
    }
}

/// Rejects `program` when `launch` cannot start one of its nodes, before any
/// node resolves or has its executable read authorized (#1736). Command
/// nodes and agents on an external CLI runtime launch a frozen process from
/// verified handles; in-process Rho agents launch none.
pub(crate) fn ensure_program_launchable<'a>(
    program: &'a crate::workflow::WorkflowProgram,
    catalog: &crate::agent::AgentCatalog,
    launch: HandleLaunch,
) -> anyhow::Result<LaunchableProgram<'a>> {
    if launch == HandleLaunch::Supported {
        return Ok(LaunchableProgram(program));
    }
    for node in program.root.nodes.values() {
        let launches_process = match &node.execution {
            NodeExecution::Command(_) => true,
            NodeExecution::Agent(agent) => catalog
                .find(&agent.agent)?
                .definition
                .runtime
                .runtime()
                .is_external_cli(),
        };
        if launches_process {
            launch.ensure()?;
        }
    }
    Ok(LaunchableProgram(program))
}

pub(crate) fn resolve_nodes_with_host(
    program: LaunchableProgram<'_>,
    host: &dyn PlanHost,
) -> anyhow::Result<BTreeMap<crate::workflow::NodeId, ResolvedNode>> {
    program
        .program()
        .root
        .nodes
        .iter()
        .map(|(id, node)| {
            let resolved = match &node.execution {
                NodeExecution::Agent(agent) => {
                    let entry = host.catalog().find(&agent.agent)?;
                    let bound = AgentBinder::bind_with_credentials(
                        Arc::new(entry.definition.clone()),
                        AgentInvocation {
                            role: AgentRole::Workflow,
                            available_tools: host.available_tools().clone(),
                        },
                        host.config(),
                        /*model_override*/ None,
                        host.credentials(),
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
    let source_origin = entry.metadata.origin.as_str();
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
        }
        | BoundRuntime::Cursor {
            permission_mode, ..
        }
        | BoundRuntime::Antigravity {
            permission_mode, ..
        } => permission_mode.to_string(),
    };
    let common = ResolvedAgent {
        agent_id: entry.definition.id.to_string(),
        fingerprint: entry.fingerprint.to_string(),
        runtime: match bound.runtime() {
            BoundRuntime::Rho { .. } => crate::workflow::AgentRuntime::Rho,
            BoundRuntime::ClaudeCli { .. } => crate::workflow::AgentRuntime::ClaudeCli,
            BoundRuntime::Cursor { .. } => crate::workflow::AgentRuntime::Cursor,
            BoundRuntime::Antigravity { .. } => crate::workflow::AgentRuntime::Antigravity,
        },
        source_origin,
        // Workflow-local agents ship with the workflow source the user planned.
        // Project catalog agents still require project trust, and project
        // plugin agents project plugin trust.
        trust_required: entry.metadata.origin == AgentOrigin::Project,
        plugin_trust_required: entry.metadata.origin == AgentOrigin::ProjectPlugin,
        prompt_policy,
        provider: None,
        model: None,
        reasoning: None,
        step_limit: sdk_config::run_step_limit().get() as u64,
        capabilities: BTreeSet::new(),
        permission_ceiling,
        auth_profile: None,
        fast: false,
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
            fast: config.fast_mode,
            ..common
        },
        BoundRuntime::ClaudeCli {
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            reasoning,
        } => {
            let (executable, executable_identity) =
                host.resolve_executable(crate::claude_runtime::executable::ADAPTER_PROGRAM)?;
            crate::claude_runtime::fence::ClaudeFence::new(
                *permission_mode,
                tools,
                *inherit_claude_config,
            )?;
            ResolvedAgent {
                model: model.clone(),
                reasoning: reasoning.map(|level| level.to_string()),
                step_limit: *max_turns,
                capabilities: tools.iter().cloned().collect(),
                executable: Some(crate::paths::display(&executable)),
                executable_identity: Some(executable_identity),
                // The adapter takes no arguments; the session carries identity.
                arguments: Vec::new(),
                ..common
            }
        }
        BoundRuntime::Cursor {
            model,
            tools,
            permission_mode,
        } => {
            let (executable, executable_identity) =
                host.resolve_executable(crate::cursor_runtime::models::CURSOR_PROGRAM)?;
            let allowed =
                crate::cursor_runtime::spawn::map_permission_mode(*permission_mode, tools)?;
            let capabilities = allowed
                .tools()
                .iter()
                .map(|tool| tool.as_flag().to_owned())
                .collect();
            let plan = crate::cursor_runtime::spawn::build_spawn_plan(
                &crate::cursor_runtime::spawn::CursorSpawnRequest {
                    model: model.clone(),
                    cwd: host.workspace().to_path_buf(),
                },
            );
            ResolvedAgent {
                model: model.clone(),
                capabilities,
                executable: Some(crate::paths::display(&executable)),
                executable_identity: Some(executable_identity),
                arguments: plan.args,
                ..common
            }
        }
        BoundRuntime::Antigravity {
            model,
            tools,
            permission_mode,
        } => {
            let (executable, executable_identity) =
                host.resolve_executable(crate::antigravity_runtime::executable::AGY_PROGRAM)?;
            let fence =
                crate::antigravity_runtime::fence::AntigravityFence::new(*permission_mode, tools)?;
            ResolvedAgent {
                model: model.clone(),
                capabilities: fence
                    .tools()
                    .iter()
                    .map(|tool| tool.as_name().to_owned())
                    .collect(),
                executable: Some(crate::paths::display(&executable)),
                executable_identity: Some(executable_identity),
                // Unused at launch (the policy rebuilds argv and adds the
                // run's own agent flags); recorded so the plan shows what will
                // run.
                arguments: crate::antigravity_runtime::executable::SERVER_ARGS
                    .iter()
                    .map(|arg| (*arg).to_owned())
                    .collect(),
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
                ToolCapability::Advisor
                    | ToolCapability::Agent
                    | ToolCapability::Agents
                    | ToolCapability::Questionnaire
                    | ToolCapability::Rho
                    | ToolCapability::Workflow
            )
        })
        .map(|capability| capability.as_str().to_owned())
        .collect()
}

/// `executable` canonicalized: a bare name found on `search_path`, a
/// relative path under `workspace`.
fn resolve_executable_path(
    executable: &str,
    workspace: &Path,
    search_path: Option<&OsStr>,
) -> anyhow::Result<PathBuf> {
    let path = Path::new(executable);
    // Every branch canonicalizes: validation requires the frozen path to equal
    // the identity's canonical path, and PATH entries are often symlinks
    // (`~/.local/bin/cursor-agent -> versions/<v>/cursor-agent`).
    let resolved = if path.components().count() == 1 {
        crate::executable::find_on(executable, search_path)
            .ok_or_else(|| anyhow::anyhow!("executable '{executable}' was not found on PATH"))?
            .canonicalize()?
    } else if path.is_absolute() {
        path.canonicalize()?
    } else {
        workspace.join(path).canonicalize()?
    };
    Ok(resolved)
}

#[cfg(test)]
#[path = "plan_host_tests.rs"]
mod tests;
