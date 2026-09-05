//! Composes selected SDK tool bundles and shuts them down through one contract.

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use rho_sdk::tool::Tool;

use crate::{
    agent::{AgentCapabilities, ToolCapability},
    config::Config,
    diagnostics::RuntimeDiagnostics,
};

use super::{
    advisor::AdvisorSessionStore,
    agent::{
        BackgroundSubagents, DelegationBundleOptions, DelegationToolSelection, SubagentManager,
    },
};

/// A feature-owned group of tools and any resources they need.
///
/// Bundles keep lifecycle ownership in the feature that creates each tool. The
/// boxed future is `Send` so callers can shut bundles down from async runtimes.
pub(crate) trait ToolBundle: Send + Sync {
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

#[derive(Clone, Debug)]
pub struct DelegationConfig {
    cwd: PathBuf,
    config_path: PathBuf,
    background: BackgroundSubagents,
    /// Catalog already discovered for `cwd`; the agent tool rediscovers when
    /// absent.
    catalog: Option<Arc<crate::agent::AgentCatalog>>,
}

impl DelegationConfig {
    pub fn new(
        cwd: PathBuf,
        config_path: PathBuf,
        background: BackgroundSubagents,
        catalog: Option<Arc<crate::agent::AgentCatalog>>,
    ) -> Self {
        Self {
            cwd,
            config_path,
            background,
            catalog,
        }
    }
}

#[derive(Clone)]
pub struct ToolSetOptions {
    capabilities: AgentCapabilities,
    advisor: Option<AdvisorSessionStore>,
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
            advisor: None,
            delegation: None,
            workflow: None,
            workflow_tracker: super::workflow_tracker::WorkflowRunTracker::new(),
        }
    }

    /// Supplies the session store the `advisor` tool reads. Without it the
    /// capability alone registers no tool, so runs with no live session to
    /// review cannot offer the advisor.
    ///
    /// The store is kept whether or not advisor mode is on, so a later
    /// `/advisor on` registers the tool without rebuilding the tool set.
    pub fn advisor(mut self, store: AdvisorSessionStore) -> Self {
        self.advisor = Some(store);
        self
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

/// The `advisor` tool and the store it reads, held whether or not the tool is
/// currently advertised.
///
/// Advisor mode toggles mid-session, and the executor must never see a tool the
/// run does not have, so registration is a state transition on the built tool
/// set rather than a reason to rebuild it.
struct AdvisorTools {
    store: AdvisorSessionStore,
    tool: Arc<dyn Tool>,
    registered: bool,
}

pub struct AppToolSet {
    tools: Vec<Arc<dyn Tool>>,
    bundles: Vec<Box<dyn ToolBundle>>,
    advisor: Option<AdvisorTools>,
    subagents: Option<SubagentManager>,
    processes: Option<super::process::ProcessManager>,
    workflow_tracker: super::workflow_tracker::WorkflowRunTracker,
    checkpoint_tracker: Arc<crate::session::workspace_checkpoint::WorkspaceCheckpointTracker>,
    web_access: super::web::WebAccessStore,
    mcp_report: super::mcp::McpSessionReport,
    mcp_catalog: super::mcp::McpCatalog,
    file_view: rho_tools::FileViewPolicy,
}

impl AppToolSet {
    pub fn disabled() -> Self {
        Self {
            tools: Vec::new(),
            bundles: Vec::new(),
            advisor: None,
            subagents: None,
            processes: None,
            workflow_tracker: super::workflow_tracker::WorkflowRunTracker::new(),
            checkpoint_tracker: Arc::new(
                crate::session::workspace_checkpoint::WorkspaceCheckpointTracker::new(false),
            ),
            web_access: super::web::WebAccessStore::new(),
            mcp_report: super::mcp::McpSessionReport::default(),
            mcp_catalog: super::mcp::McpCatalog::default(),
            file_view: rho_tools::FileViewPolicy::default(),
        }
    }

    pub fn new(config: &Config, diagnostics: RuntimeDiagnostics, options: ToolSetOptions) -> Self {
        let ToolSetOptions {
            capabilities,
            advisor,
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
        let process_environment =
            rho_sdk::ProcessEnvironment::inherit_except(rho_providers::credential_env_vars());

        let edit_format = config.resolved_edit_tool();
        tool_set.file_view.set(edit_format);
        tool_set.add_bundle(super::coding::sdk_bundle(
            &capabilities,
            config.max_output_bytes,
            process_environment.clone(),
            tool_set.checkpoint_tracker.clone(),
            tool_set.file_view.clone(),
        ));
        if capabilities.contains(&ToolCapability::WriteFile) {
            tool_set.add_bundle(super::save_agent::sdk_bundle(config.max_output_bytes));
        }
        if capabilities.contains(&ToolCapability::Process) {
            let bundle = super::process::sdk_bundle(
                config.max_output_bytes,
                process_environment.clone(),
                tool_set.checkpoint_tracker.clone(),
            );
            tool_set.processes = Some(bundle.manager_handle());
            tool_set.add_bundle(bundle);
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
        if let (true, Some(store)) = (capabilities.contains(&ToolCapability::Advisor), advisor) {
            // The tool set owns advisor initialization: the model and the
            // registration state both come from the same config read.
            store.set_model(super::advisor::advisor_model(config).cloned());
            tool_set.advisor = Some(AdvisorTools {
                tool: super::advisor::advisor_tool(store.clone()),
                store,
                registered: false,
            });
            tool_set.set_advisor_registered(super::advisor::advisor_available(config));
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
            if let Some(bundle) = super::tui_fixture::sdk_bundle(tool_set.processes.clone()) {
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
                    catalog: delegation.catalog,
                },
                tool_set.checkpoint_tracker.clone(),
            );
            tool_set.subagents = Some(bundle.manager_handle());
            tool_set.add_bundle(bundle);
        }

        tool_set
    }

    /// Attach MCP inventory and any live tool bundle from session assembly.
    pub(crate) fn with_mcp(mut self, outcome: super::mcp::McpConnectOutcome) -> Self {
        self.attach_mcp(outcome);
        self
    }

    /// Attach a finished MCP connect onto an already-built tool set.
    pub(crate) fn attach_mcp(&mut self, outcome: super::mcp::McpConnectOutcome) {
        self.mcp_report = outcome.report;
        self.mcp_catalog = outcome.catalog;
        if let Some(bundle) = outcome.bundle {
            self.add_bundle(bundle);
        }
    }

    pub(crate) fn add_bundle(&mut self, bundle: impl ToolBundle + 'static) {
        self.tools.extend(bundle.tools().iter().cloned());
        self.bundles.push(Box::new(bundle));
    }

    /// Prompts and resources connected servers offer the user.
    pub(crate) fn mcp_catalog(&self) -> &super::mcp::McpCatalog {
        &self.mcp_catalog
    }

    pub(crate) fn mcp_report(&self) -> &super::mcp::McpSessionReport {
        &self.mcp_report
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

    /// The advisor's session store, present whenever the run may offer the
    /// advisor, even while advisor mode is off.
    pub fn advisor(&self) -> Option<&AdvisorSessionStore> {
        self.advisor.as_ref().map(|advisor| &advisor.store)
    }

    /// Whether the `advisor` tool is currently advertised to the model.
    pub fn advisor_registered(&self) -> bool {
        self.advisor
            .as_ref()
            .is_some_and(|advisor| advisor.registered)
    }

    /// Adds or removes the `advisor` tool, so `/advisor` reaches the next
    /// runtime build without disturbing tools that hold live state.
    ///
    /// Returns whether the advertised tool list changed; callers rebuild the
    /// runtime only then.
    pub fn set_advisor_registered(&mut self, registered: bool) -> bool {
        let Some(advisor) = self.advisor.as_mut() else {
            return false;
        };
        if advisor.registered == registered {
            return false;
        }
        advisor.registered = registered;
        let tool = Arc::clone(&advisor.tool);
        if registered {
            self.tools.push(tool);
        } else {
            self.tools.retain(|existing| !Arc::ptr_eq(existing, &tool));
        }
        true
    }

    /// Replaces the advertised built-in file edit tool.
    ///
    /// Returns the previous format when the advertised tool list changed so
    /// callers can rebuild the runtime and tell the model. No-ops when this
    /// run has no edit tool or the selection is already active.
    ///
    /// Matches built-in edit tool names (`edit`, `apply_patch`, `str_replace`).
    pub fn set_edit_tool(
        &mut self,
        edit_tool: rho_tools::EditFormat,
        max_output_bytes: usize,
    ) -> Option<rho_tools::EditFormat> {
        let previous = self.edit_tool()?;
        if previous == edit_tool {
            return None;
        }
        let position = self
            .tools
            .iter()
            .position(|tool| rho_tools::EditFormat::is_edit_tool_name(tool.spec().name.as_str()))?;
        let mutation_observer: Arc<dyn rho_tools::WorkspaceMutationObserver> =
            self.checkpoint_tracker.clone();
        self.tools[position] =
            super::coding::edit_tool(edit_tool, max_output_bytes, mutation_observer);
        self.file_view.set(edit_tool);
        Some(previous)
    }

    #[cfg(test)]
    pub(crate) fn file_view_style(&self) -> rho_tools::FileViewStyle {
        self.file_view.style()
    }

    /// The currently advertised built-in edit format, when this run exposes one.
    pub fn edit_tool(&self) -> Option<rho_tools::EditFormat> {
        self.tools
            .iter()
            .find_map(|tool| rho_tools::EditFormat::from_tool_name(tool.spec().name.as_str()))
    }

    pub fn subagents(&self) -> Option<&SubagentManager> {
        self.subagents.as_ref()
    }

    pub fn processes(&self) -> Option<&super::process::ProcessManager> {
        self.processes.as_ref()
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
