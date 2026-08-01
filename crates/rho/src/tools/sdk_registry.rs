//! Composes selected SDK tool bundles and shuts them down through one contract.

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use rho_sdk::tool::Tool;

use crate::{
    agent::{AgentCapabilities, ToolCapability},
    config::Config,
    diagnostics::RuntimeDiagnostics,
};

use super::agent::{
    BackgroundSubagents, DelegationBundleOptions, DelegationToolSelection, SubagentManager,
};

/// A feature-owned group of tools and any resources they need.
///
/// Bundles keep lifecycle ownership in the feature that creates each tool. The
/// boxed future is `Send` so callers can shut bundles down from async runtimes.
pub(super) trait ToolBundle: Send + Sync {
    fn tools(&self) -> &[Arc<dyn Tool>];

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// A bundle for features which need no shutdown work.
pub(crate) struct StaticToolBundle {
    tools: Vec<Arc<dyn Tool>>,
}

impl StaticToolBundle {
    pub(crate) fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self { tools }
    }
}

impl ToolBundle for StaticToolBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationConfig {
    cwd: PathBuf,
    config_path: PathBuf,
    background: BackgroundSubagents,
}

impl DelegationConfig {
    pub fn new(cwd: PathBuf, config_path: PathBuf, background: BackgroundSubagents) -> Self {
        Self {
            cwd,
            config_path,
            background,
        }
    }
}

#[derive(Clone)]
pub struct ToolSetOptions {
    capabilities: AgentCapabilities,
    delegation: Option<DelegationConfig>,
    workflow: Option<Arc<dyn super::workflow::WorkflowToolService>>,
    workflow_tracker: super::workflow_tracker::WorkflowRunTracker,
}

impl Default for ToolSetOptions {
    fn default() -> Self {
        Self::new(AgentCapabilities::all_host_tools())
    }
}

impl ToolSetOptions {
    pub fn new(capabilities: AgentCapabilities) -> Self {
        Self {
            capabilities,
            delegation: None,
            workflow: None,
            workflow_tracker: super::workflow_tracker::WorkflowRunTracker::new(),
        }
    }

    pub fn delegation(mut self, config: DelegationConfig) -> Self {
        self.delegation = Some(config);
        self
    }

    pub(crate) fn workflow(
        mut self,
        service: Arc<dyn super::workflow::WorkflowToolService>,
    ) -> Self {
        self.workflow = Some(service);
        self
    }

    pub(crate) fn workflow_tracker(
        mut self,
        tracker: super::workflow_tracker::WorkflowRunTracker,
    ) -> Self {
        self.workflow_tracker = tracker;
        self
    }
}

pub struct AppToolSet {
    tools: Vec<Arc<dyn Tool>>,
    bundles: Vec<Box<dyn ToolBundle>>,
    subagents: Option<SubagentManager>,
    workflow_tracker: super::workflow_tracker::WorkflowRunTracker,
    checkpoint_tracker: Arc<crate::session::workspace_checkpoint::WorkspaceCheckpointTracker>,
    web_access: super::web::WebAccessStore,
}

impl AppToolSet {
    pub fn disabled() -> Self {
        Self {
            tools: Vec::new(),
            bundles: Vec::new(),
            subagents: None,
            workflow_tracker: super::workflow_tracker::WorkflowRunTracker::new(),
            checkpoint_tracker: Arc::new(
                crate::session::workspace_checkpoint::WorkspaceCheckpointTracker::new(false),
            ),
            web_access: super::web::WebAccessStore::new(),
        }
    }

    pub fn new(config: &Config, diagnostics: RuntimeDiagnostics, options: ToolSetOptions) -> Self {
        let ToolSetOptions {
            capabilities,
            delegation,
            workflow,
            workflow_tracker,
        } = options;
        let mut tool_set = Self::disabled();
        tool_set.workflow_tracker = workflow_tracker;
        tool_set.checkpoint_tracker = Arc::new(
            crate::session::workspace_checkpoint::WorkspaceCheckpointTracker::new(
                config.experimental_workspace_rewind,
            ),
        );
        // Compose one child-process environment policy for every process tool.
        // Provider credential env vars are excluded so agent commands cannot
        // read host API keys from the ambient environment.
        let process_environment = rho_sdk::ProcessEnvironment::inherit_except(
            rho_providers::credential_env_vars().iter().copied(),
        );

        tool_set.add_bundle(super::coding::sdk_bundle(
            &capabilities,
            config.max_output_bytes,
            process_environment.clone(),
            tool_set.checkpoint_tracker.clone(),
        ));
        if capabilities.contains(&ToolCapability::Process) {
            tool_set.add_bundle(super::process::sdk_bundle(
                config.max_output_bytes,
                process_environment.clone(),
                tool_set.checkpoint_tracker.clone(),
            ));
        }
        if capabilities.contains(&ToolCapability::Skill) {
            tool_set.add_bundle(super::sdk_features::skill_bundle(config.max_output_bytes));
        }
        if capabilities.contains(&ToolCapability::Rho) {
            tool_set.add_bundle(super::rho::sdk_bundle(diagnostics, config.max_output_bytes));
        }
        if capabilities.contains(&ToolCapability::Questionnaire) {
            tool_set.add_bundle(super::sdk_features::questionnaire_bundle());
        }
        if let (true, Some(service)) = (capabilities.contains(&ToolCapability::Workflow), workflow)
        {
            tool_set.add_bundle(super::workflow::sdk_bundle(
                service,
                config.max_output_bytes,
            ));
        }
        #[cfg(debug_assertions)]
        if capabilities.contains(&ToolCapability::Extension(super::tui_fixture::NAME.into())) {
            if let Some(bundle) = super::tui_fixture::sdk_bundle() {
                tool_set.add_bundle(bundle);
            }
        }
        let web_access = tool_set.web_access.clone();
        tool_set.add_bundle(super::web::sdk_bundle(
            config,
            &capabilities,
            process_environment,
            web_access,
        ));

        let delegation_tools = DelegationToolSelection::from_capabilities(&capabilities);
        if let (Some(selection), Some(delegation)) = (delegation_tools, delegation) {
            let bundle = super::agent::sdk_bundle(
                config,
                DelegationBundleOptions {
                    cwd: delegation.cwd,
                    tools: selection,
                    config_path: delegation.config_path,
                    background: delegation.background,
                },
                tool_set.checkpoint_tracker.clone(),
            );
            tool_set.subagents = Some(bundle.manager_handle());
            tool_set.add_bundle(bundle);
        }

        tool_set
    }

    fn add_bundle(&mut self, bundle: impl ToolBundle + 'static) {
        self.tools.extend(bundle.tools().iter().cloned());
        self.bundles.push(Box::new(bundle));
    }

    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    pub fn specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }

    /// Returns registry names without applying any additional capability filter.
    pub fn unfiltered_names(&self) -> impl Iterator<Item = String> + '_ {
        self.tools.iter().map(|tool| tool.spec().name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.unfiltered_names().any(|registered| registered == name)
    }

    pub fn subagents(&self) -> Option<&SubagentManager> {
        self.subagents.as_ref()
    }

    pub fn workflow_tracker(&self) -> &super::workflow_tracker::WorkflowRunTracker {
        &self.workflow_tracker
    }

    pub fn checkpoint_tracker(
        &self,
    ) -> &Arc<crate::session::workspace_checkpoint::WorkspaceCheckpointTracker> {
        &self.checkpoint_tracker
    }

    pub fn web_access(&self) -> &super::web::WebAccessStore {
        &self.web_access
    }

    pub async fn shutdown(&self) {
        for bundle in &self.bundles {
            bundle.shutdown().await;
        }
    }
}

#[cfg(test)]
#[path = "sdk_registry_tests.rs"]
mod tests;
