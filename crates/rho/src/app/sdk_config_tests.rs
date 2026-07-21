use std::path::Path;

use super::{RuntimeOptions, SdkBootstrapOptions, ToolOptions, WorkspaceOptions};
use pretty_assertions::assert_eq;
use {
    crate::compaction::CompactionConfig, crate::config::Config,
    rho_providers::providers::ProviderBuildOptions,
};

#[test]
fn converts_application_config_without_credentials_or_side_effects() {
    let config = Config {
        provider: "anthropic".into(),
        model: "claude-test".into(),
        max_output_bytes: 1234,
        max_tool_output_lines: 45,
        rtk: false,
        inline_shell: "zsh".into(),
        auto_compact: true,
        compact_threshold_percent: 75,
        compact_target_percent: 40,
        ..Config::default()
    };

    let actual = SdkBootstrapOptions::from_config(&config, Path::new("workspace")).unwrap();

    assert_eq!(
        actual,
        SdkBootstrapOptions {
            provider: ProviderBuildOptions::new("anthropic", "claude-test", config.reasoning,)
                .unwrap(),
            runtime: RuntimeOptions {
                reasoning: config.reasoning,
                compaction: CompactionConfig {
                    auto_compact: true,
                    threshold_percent: 75,
                    target_percent: 40,
                },
            },
            workspace: WorkspaceOptions {
                root: "workspace".into(),
            },
            tools: ToolOptions {
                max_output_bytes: 1234,
                max_output_lines: 45,
                rtk_enabled: false,
                inline_shell: "zsh".into(),
            },
        }
    );
}

#[test]
fn passes_configured_ollama_base_to_provider_build_options() {
    let mut config = Config {
        provider: "ollama".into(),
        model: "local-model".into(),
        auth: "none".into(),
        ..Config::default()
    };
    config.providers.ollama.base_url = "http://ollama.internal:22000/v1".parse().unwrap();

    let actual = SdkBootstrapOptions::from_config(&config, Path::new("workspace")).unwrap();
    let expected = ProviderBuildOptions::new("ollama", "local-model", config.reasoning)
        .unwrap()
        .endpoint(config.providers.ollama.base_url.clone())
        .unwrap();

    assert_eq!(actual.provider, expected);
}
