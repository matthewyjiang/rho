use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use rho_sdk::{tool::ToolContext, CapabilityRequest, CapabilitySource, PathScope};

use super::{
    executable_candidates, path_scope, project_agent_catalogs_trusted, AppWorkflowToolService,
};

impl AppWorkflowToolService {
    pub(super) async fn authorized_config(
        &self,
        context: &ToolContext,
        path: &Path,
    ) -> anyhow::Result<crate::config::Config> {
        self.authorize_read(context, path, PathScope::UnrestrictedFilesystem)
            .await?;
        let opened =
            match crate::workflow::VerifiedPath::open(path, crate::workflow::ContentHash::Skip) {
                Ok(opened) => opened,
                Err(crate::workflow::WorkflowError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(crate::config::Config::default());
                }
                Err(error) => return Err(error.into()),
            };
        self.authorize_opened_identity(context, &opened.identity)
            .await?;
        let text = opened.read_utf8()?;
        crate::config::Config::parse_settings(&text)
    }

    pub(super) async fn authorize_path(
        &self,
        context: &ToolContext,
        path: &Path,
    ) -> anyhow::Result<crate::workflow::VerifiedPath> {
        let lexical = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        if !lexical.starts_with(&self.cwd) {
            anyhow::bail!("workflow source is outside the workspace");
        }
        context
            .authorize(CapabilityRequest::read_path(
                &lexical,
                PathScope::PrimaryWorkspace,
                CapabilitySource::built_in_tool("workflow"),
            ))
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let workspace = context
            .workspace()
            .ok_or_else(|| anyhow::anyhow!("workflow tool requires a workspace"))?;
        let resolved = workspace
            .resolve_for_read(path)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if !resolved.path().starts_with(&self.cwd) {
            anyhow::bail!("workflow source is outside the workspace");
        }
        context
            .authorize(CapabilityRequest::read_path(
                resolved.path(),
                resolved.scope().clone(),
                CapabilitySource::built_in_tool("workflow"),
            ))
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if resolved.path() != lexical {
            return Err(crate::workflow::WorkflowError::SourceSymlink { path: lexical }.into());
        }
        let opened = crate::workflow::VerifiedPath::open(
            resolved.path(),
            crate::workflow::ContentHash::Skip,
        )?;
        self.authorize_opened_identity(context, &opened.identity)
            .await?;
        if Path::new(&opened.identity.canonical_path) != resolved.path() {
            return Err(crate::workflow::WorkflowError::SourceSymlink {
                path: resolved.path().to_path_buf(),
            }
            .into());
        }
        Ok(opened)
    }

    pub(super) async fn authorized_agent_catalog(
        &self,
        context: &ToolContext,
        workflow_entry: &Path,
    ) -> anyhow::Result<crate::agent::AgentCatalog> {
        let home = crate::paths::home_dir();
        let mut sources = crate::agent::AgentCatalogSources::default();
        if let Some(home) = home.as_deref() {
            sources.agents_home = self
                .authorized_agent_sources(context, &home.join(".agents/agents"))
                .await?;
            sources.rho_home = self
                .authorized_agent_sources(context, &home.join(".rho/agents"))
                .await?;
        }
        if project_agent_catalogs_trusted() {
            for root in crate::workspace::project_ancestor_dirs(&self.cwd)
                .into_iter()
                .map(|path| path.join(".agents/agents"))
            {
                sources
                    .project
                    .push(self.authorized_agent_sources(context, &root).await?);
            }
        }
        let workflow_agents = {
            let root = crate::agent::workflow_local_agents_root(workflow_entry);
            if root.is_absolute() {
                root
            } else {
                self.cwd.join(root)
            }
        };
        sources.workflow = self
            .authorized_agent_sources(context, &workflow_agents)
            .await?;
        crate::agent::AgentCatalog::from_authorized_sources(sources).map_err(Into::into)
    }

    async fn authorized_agent_sources(
        &self,
        context: &ToolContext,
        root: &Path,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let scope = path_scope(&self.cwd, root);
        self.authorize_read(context, root, scope.clone()).await?;
        let opened_root = match crate::workflow::open_verified_directory(root) {
            Ok(opened) => opened,
            Err(crate::workflow::WorkflowError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error.into()),
        };
        self.authorize_opened_identity(context, &opened_root.identity)
            .await?;
        let mut names = crate::workflow::opened_directory_names(&opened_root)?;
        names.retain(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "md")
        });
        names.sort();
        let mut sources = Vec::new();
        for name in names {
            let path = Path::new(&opened_root.identity.canonical_path).join(&name);
            self.authorize_read(context, &path, scope.clone()).await?;
            let opened = crate::workflow::open_verified_file_in_directory(
                &opened_root,
                Path::new(&name),
                crate::workflow::ContentHash::Skip,
            )?;
            self.authorize_opened_identity(context, &opened.identity)
                .await?;
            let source = opened.read_utf8()?;
            sources.push((path, source));
        }
        Ok(sources)
    }

    pub(super) async fn authorize_node_resolution_reads(
        &self,
        graph: &crate::workflow::WorkflowGraph,
        catalog: &crate::agent::AgentCatalog,
        context: &ToolContext,
    ) -> anyhow::Result<std::collections::BTreeMap<String, crate::workflow::ExecutableIdentity>>
    {
        let mut executables = BTreeSet::new();
        let mut directories = BTreeSet::new();
        for node in graph.nodes.values() {
            match &node.execution {
                crate::workflow::NodeExecution::Command(command) => {
                    let (executable, cwd) = match command {
                        crate::workflow::CommandNode::Direct {
                            executable, cwd, ..
                        }
                        | crate::workflow::CommandNode::Shell {
                            executable, cwd, ..
                        } => (executable, cwd),
                    };
                    executables.insert(executable.clone());
                    directories.insert(self.cwd.join(cwd));
                }
                crate::workflow::NodeExecution::Agent(agent) => {
                    let entry = catalog.find(&agent.agent)?;
                    if matches!(
                        entry.definition.runtime,
                        crate::agent::AgentRuntimeSpec::ClaudeCli(_)
                    ) {
                        executables.insert("claude".to_owned());
                    }
                }
            }
        }
        for directory in directories {
            self.authorize_workspace_identity_path(context, &directory)
                .await?;
        }
        let mut identities = std::collections::BTreeMap::new();
        for executable in executables {
            let opened = self
                .authorize_opened_executable_resolution(context, &executable)
                .await?;
            let interpreter = match opened.interpreter_request.as_ref() {
                Some(crate::workflow::ExecutableInterpreterRequest::Absolute(path)) => {
                    Some(self.authorize_opened_executable_path(context, path).await?)
                }
                Some(crate::workflow::ExecutableInterpreterRequest::Search(program)) => Some(
                    self.authorize_opened_executable_resolution(context, program)
                        .await?,
                ),
                None => None,
            };
            let interpreter = interpreter
                .map(crate::workflow::OpenedExecutable::into_binary)
                .transpose()?;
            identities.insert(
                executable,
                crate::workflow::freeze_opened_executable(opened, interpreter)?,
            );
        }
        Ok(identities)
    }

    async fn authorize_opened_executable_resolution(
        &self,
        context: &ToolContext,
        executable: &str,
    ) -> anyhow::Result<crate::workflow::OpenedExecutable> {
        let path = Path::new(executable);
        if path.components().count() == 1 {
            let candidates = executable_candidates(executable);
            for candidate in &candidates {
                self.authorize_read(context, candidate, path_scope(&self.cwd, candidate))
                    .await?;
            }
            for candidate in candidates {
                match crate::workflow::open_executable_candidate(&candidate) {
                    Ok(Some(opened)) => {
                        self.authorize_read(context, &candidate, path_scope(&self.cwd, &candidate))
                            .await?;
                        self.authorize_opened_identity(context, opened.identity())
                            .await?;
                        return Ok(opened);
                    }
                    Ok(None) => {}
                    Err(crate::workflow::WorkflowError::Io(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            anyhow::bail!("executable '{executable}' was not found on PATH");
        } else {
            let lexical = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.cwd.join(path)
            };
            self.authorize_read(context, &lexical, path_scope(&self.cwd, &lexical))
                .await?;
            let opened = crate::workflow::open_executable(&lexical)?;
            self.authorize_opened_identity(context, opened.identity())
                .await?;
            Ok(opened)
        }
    }

    async fn authorize_opened_executable_path(
        &self,
        context: &ToolContext,
        path: &Path,
    ) -> anyhow::Result<crate::workflow::OpenedExecutable> {
        self.authorize_read(context, path, path_scope(&self.cwd, path))
            .await?;
        let opened = crate::workflow::open_executable(path)?;
        self.authorize_opened_identity(context, opened.identity())
            .await?;
        Ok(opened)
    }

    async fn authorize_workspace_identity_path(
        &self,
        context: &ToolContext,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.authorize_identity_path(context, path, PathScope::PrimaryWorkspace)
            .await
    }

    async fn authorize_identity_path(
        &self,
        context: &ToolContext,
        path: &Path,
        scope: PathScope,
    ) -> anyhow::Result<()> {
        self.authorize_read(context, path, scope).await?;
        let canonical = path.canonicalize()?;
        if canonical != path {
            self.authorize_read(context, &canonical, path_scope(&self.cwd, &canonical))
                .await?;
        }
        Ok(())
    }

    async fn authorize_opened_identity(
        &self,
        context: &ToolContext,
        identity: &crate::workflow::FrozenPathIdentity,
    ) -> anyhow::Result<()> {
        let canonical = Path::new(&identity.canonical_path);
        self.authorize_read(context, canonical, path_scope(&self.cwd, canonical))
            .await?;
        Ok(())
    }

    async fn authorize_read(
        &self,
        context: &ToolContext,
        path: &Path,
        scope: PathScope,
    ) -> anyhow::Result<()> {
        context
            .authorize(CapabilityRequest::read_path(
                path,
                scope,
                CapabilitySource::built_in_tool("workflow"),
            ))
            .await
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}
