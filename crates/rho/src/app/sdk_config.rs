use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use url::Url;

use {
    crate::compaction::CompactionConfig, crate::config::Config, rho_providers::model::ModelError,
    rho_providers::providers::ProviderBuildOptions, rho_providers::reasoning::ReasoningLevel,
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
        Self::from_config_with_provider_transport(config, workspace_root, None, None)
    }

    pub(crate) fn from_config_with_provider_transport(
        config: &Config,
        workspace_root: &Path,
        endpoint: Option<Url>,
        request_timeout: Option<Duration>,
    ) -> Result<Self, ModelError> {
        let mut provider =
            ProviderBuildOptions::new(&config.provider, &config.model, config.reasoning)?;
        if let Some(endpoint) = endpoint {
            provider = provider.endpoint(endpoint)?;
        }
        if let Some(request_timeout) = request_timeout {
            provider = provider.request_timeout(request_timeout)?;
        }
        Ok(Self {
            provider,
            runtime: RuntimeOptions {
                reasoning: config.reasoning,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeOptions {
    pub(crate) reasoning: ReasoningLevel,
    pub(crate) compaction: CompactionConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceOptions {
    pub(crate) root: PathBuf,
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
