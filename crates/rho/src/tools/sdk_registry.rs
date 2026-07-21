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
pub(super) struct StaticToolBundle {
    tools: Vec<Arc<dyn Tool>>,
}

impl StaticToolBundle {
    pub(super) fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSetOptions {
    capabilities: AgentCapabilities,
    delegation: Option<DelegationConfig>,
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
        }
    }

    pub fn delegation(mut self, config: DelegationConfig) -> Self {
        self.delegation = Some(config);
        self
    }
}

pub struct AppToolSet {
    tools: Vec<Arc<dyn Tool>>,
    bundles: Vec<Box<dyn ToolBundle>>,
    subagents: Option<SubagentManager>,
}

impl AppToolSet {
    pub fn disabled() -> Self {
        Self {
            tools: Vec::new(),
            bundles: Vec::new(),
            subagents: None,
        }
    }

    pub fn new(config: &Config, diagnostics: RuntimeDiagnostics, options: ToolSetOptions) -> Self {
        let ToolSetOptions {
            capabilities,
            delegation,
        } = options;
        let mut tool_set = Self::disabled();

        tool_set.add_bundle(super::coding::sdk_bundle(
            &capabilities,
            config.max_output_bytes,
        ));
        if capabilities.contains(&ToolCapability::Process) {
            tool_set.add_bundle(super::process::sdk_bundle(config.max_output_bytes));
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
        #[cfg(debug_assertions)]
        if capabilities.contains(&ToolCapability::Extension(super::tui_fixture::NAME.into())) {
            if let Some(bundle) = super::tui_fixture::sdk_bundle() {
                tool_set.add_bundle(bundle);
            }
        }
        tool_set.add_bundle(super::web::sdk_bundle(config, &capabilities));

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

    pub fn contains(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.spec().name == name)
    }

    pub fn subagents(&self) -> Option<&SubagentManager> {
        self.subagents.as_ref()
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
