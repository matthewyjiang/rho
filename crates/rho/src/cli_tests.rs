use clap::Parser;

use super::*;

#[test]
fn parses_new_provider_auth_modes() {
    for auth in ["moonshot-api-key", "kimi-oauth", "xai-api-key", "xai-oauth"] {
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
