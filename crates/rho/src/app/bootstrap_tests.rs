use clap::Parser;

use crate::{cli::Cli, config::Config, model::ModelError};

use super::{host_capabilities, is_interactive_startup_unavailable_error, AgentRole};

#[test]
fn missing_xai_api_key_is_nonfatal_for_interactive_startup() {
    assert!(is_interactive_startup_unavailable_error(
        &ModelError::MissingXaiApiKey
    ));
}

#[test]
fn unsupported_provider_is_nonfatal_for_interactive_startup() {
    assert!(is_interactive_startup_unavailable_error(
        &ModelError::UnsupportedProvider("anthropic".into())
    ));
}

#[test]
fn disabled_delegation_is_not_advertised_as_a_host_capability() {
    let cli = Cli::try_parse_from(["rho"]).unwrap();
    let config = Config {
        enable_subagents: false,
        ..Config::default()
    };

    let capabilities = host_capabilities(&cli, &config, AgentRole::InteractiveRoot);

    assert!(!capabilities.contains("agent"));
    assert!(!capabilities.contains("agents"));
}
