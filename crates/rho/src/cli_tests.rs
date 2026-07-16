use clap::Parser;

use super::*;

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
