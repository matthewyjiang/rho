use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use url::Url;

use {
    crate::compaction::CompactionConfig,
    crate::config::Config,
    rho_providers::model::ModelError,
    rho_providers::providers::ProviderBuildOptions,
    rho_providers::reasoning::ReasoningLevel,
    rho_sdk::{Workspace, WorkspacePathError},
};

/// Step budget for one application run.
///
/// The agent loop historically had no step limit; long goal-mode turns rely on
/// that. This keeps the SDK's small default from truncating turns while still
/// bounding a runaway loop.
pub(crate) fn run_step_limit() -> std::num::NonZeroUsize {
    std::num::NonZeroUsize::new(10_000).expect("step limit is nonzero")
}

/// Maximum tool calls the application may execute at once.
pub(crate) fn parallel_tool_limit() -> std::num::NonZeroUsize {
    std::num::NonZeroUsize::new(4).expect("parallel tool limit is nonzero")
}

/// Application-owned conversion from persisted Rho config to SDK bootstrap data.
///
/// These values deliberately live in `rho-coding-agent`, not `rho-sdk`. They
/// omit login, keychain, update, terminal, and persistence behavior. Credential
/// acquisition is a separate explicit bootstrap step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SdkBootstrapOptions {
    pub(crate) provider: ProviderBuildOptions,
    pub(crate) runtime: RuntimeOptions,
    pub(crate) workspace: WorkspaceOptions,
    pub(crate) tools: ToolOptions,
}

impl SdkBootstrapOptions {
    pub(crate) fn from_config(config: &Config, workspace_root: &Path) -> Result<Self, ModelError> {
        Self::from_config_with_provider_transport(
            config,
            workspace_root,
            config.resolved_provider_endpoint(&config.provider),
            None,
        )
    }

    pub(crate) fn from_config_with_provider_transport(
        config: &Config,
        workspace_root: &Path,
        endpoint: Option<Url>,
        request_timeout: Option<Duration>,
    ) -> Result<Self, ModelError> {
        let provider = provider_options_with_transport(config, endpoint, request_timeout)?;
        Ok(Self {
            provider,
            runtime: RuntimeOptions {
                reasoning: config.reasoning,
                service_tier: config
                    .fast_mode
                    .then_some(rho_sdk::model::ServiceTier::Priority),
                compaction: CompactionConfig::from(config),
            },
            workspace: WorkspaceOptions {
                root: workspace_root.to_path_buf(),
            },
            tools: ToolOptions {
                max_output_bytes: config.max_output_bytes,
                max_output_lines: config.max_tool_output_lines,
                rtk_enabled: config.rtk,
                inline_shell: config.inline_shell.clone(),
            },
        })
    }
}

/// Canonical conversion of application config into provider construction options.
///
/// Runtime provider rebuilds must use this path so transport settings such as a
/// custom endpoint are preserved across model, reasoning, and auth changes.
pub(crate) fn provider_options_from_config(
    config: &Config,
) -> Result<ProviderBuildOptions, ModelError> {
    provider_options_with_transport(
        config,
        config.resolved_provider_endpoint(&config.provider),
        None,
    )
}

fn provider_options_with_transport(
    config: &Config,
    endpoint: Option<Url>,
    request_timeout: Option<Duration>,
) -> Result<ProviderBuildOptions, ModelError> {
    let mut provider =
        ProviderBuildOptions::new(&config.provider, &config.model, config.reasoning)?
            .with_auth(&config.auth)?
            .hosted_web_search(crate::tools::web::hosted_web_search_active(config));
    if let Some(endpoint) = endpoint {
        provider = provider.endpoint(endpoint)?;
    }
    if let Some(request_timeout) = request_timeout {
        provider = provider.request_timeout(request_timeout)?;
    }
    Ok(provider)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeOptions {
    pub(crate) reasoning: ReasoningLevel,
    pub(crate) service_tier: Option<rho_sdk::model::ServiceTier>,
    pub(crate) compaction: CompactionConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceOptions {
    pub(crate) root: PathBuf,
}

impl WorkspaceOptions {
    /// Builds Rho's tool workspace with access to paths outside the working
    /// directory. Relative paths still resolve from the working directory.
    pub(crate) fn build_workspace(&self) -> Result<Workspace, WorkspacePathError> {
        Ok(Workspace::new(&self.root)?.with_unrestricted_file_access())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolOptions {
    pub(crate) max_output_bytes: usize,
    pub(crate) max_output_lines: usize,
    pub(crate) rtk_enabled: bool,
    pub(crate) inline_shell: String,
}

#[cfg(test)]
#[path = "sdk_config_tests.rs"]
mod tests;
