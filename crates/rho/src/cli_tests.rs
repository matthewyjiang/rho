use clap::Parser;

use super::*;

#[test]
fn parses_new_provider_auth_modes() {
    for auth in [
        "moonshot-api-key",
        "poolside-api-key",
        "openrouter-api-key",
        "openrouter-oauth",
        "kimi-oauth",
        "xai-api-key",
        "xai-oauth",
        "google-api-key",
    ] {
        let cli = Cli::try_parse_from(["rho", "--auth", auth]).unwrap();
        assert_eq!(cli.auth.as_deref(), Some(auth));
    }
}

#[test]
fn parses_attach_subcommand() {
    let cli = Cli::try_parse_from(["rho", "attach", "abc123"]).unwrap();

    assert!(matches!(
        cli.command,
        Some(Command::Attach { id }) if id == "abc123"
    ));
}

#[test]
fn attach_requires_an_id() {
    let error = Cli::try_parse_from(["rho", "attach"]).unwrap_err();

    assert!(error.to_string().contains("<ID>"));
}

#[test]
fn agent_selection_is_global() {
    let root = Cli::try_parse_from(["rho", "--agent", "reviewer"]).unwrap();
    assert_eq!(root.agent.as_deref(), Some("reviewer"));

    let run = Cli::try_parse_from(["rho", "run", "--agent", "worker", "ship it"]).unwrap();
    assert_eq!(run.agent.as_deref(), Some("worker"));
}

#[test]
fn parses_credential_store_commands() {
    use rho_providers::credentials::CredentialStoreBackend;

    let probe = Cli::try_parse_from(["rho", "credential-store", "probe", "os"]).unwrap();
    assert!(matches!(
        probe.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Probe { backend }
        }) if backend == CredentialStoreBackend::Os
    ));

    let probe_default = Cli::try_parse_from(["rho", "credential-store", "probe"]).unwrap();
    assert!(matches!(
        probe_default.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Probe { backend }
        }) if backend == CredentialStoreBackend::Os
    ));

    let probe_auto = Cli::try_parse_from(["rho", "credential-store", "probe", "auto"]).unwrap();
    assert!(matches!(
        probe_auto.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Probe { backend }
        }) if backend == CredentialStoreBackend::Os
    ));

    let set = Cli::try_parse_from(["rho", "credential-store", "set", "file"]).unwrap();
    assert!(matches!(
        set.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Set { backend }
        }) if backend == CredentialStoreBackend::File
    ));

    let status = Cli::try_parse_from(["rho", "credential-store", "status"]).unwrap();
    assert!(matches!(
        status.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Status
        })
    ));
}

#[test]
fn rejects_unknown_credential_store_backend() {
    assert!(Cli::try_parse_from(["rho", "credential-store", "probe", "sqlite"]).is_err());
    assert!(Cli::try_parse_from(["rho", "credential-store", "set", "sqlite"]).is_err());
}

#[test]
fn parses_structured_output_and_execution_bounds() {
    let cli = Cli::try_parse_from([
        "rho",
        "run",
        "--output",
        "jsonl",
        "--max-steps",
        "12",
        "--timeout",
        "20m",
        "ship it",
    ])
    .unwrap();

    assert!(matches!(
        cli.command,
        Some(Command::Run {
            output: OutputFormat::Jsonl,
            max_steps: Some(max_steps),
            timeout: Some(timeout),
            ..
        }) if max_steps.get() == 12 && timeout == std::time::Duration::from_secs(1_200)
    ));
}

#[test]
fn rejects_zero_steps_and_invalid_durations() {
    for arguments in [
        ["rho", "run", "--max-steps", "0"],
        ["rho", "run", "--timeout", "soon"],
    ] {
        assert!(Cli::try_parse_from(arguments).is_err());
    }
}
