use std::time::Duration;

use clap::Parser;
use pretty_assertions::assert_eq;

use super::reexec_args;
use crate::{
    cli::{Cli, Command, DaemonCommand},
    daemon::Handoff,
};

// Covers: the arguments a daemon re-executes itself with parse back to the
// same `rho daemon run`, carrying its idle period and the descriptors it
// hands over, so a renamed option or a new one cannot silently drop out of
// the restart.
// Owner: daemon restart in place.
#[test]
fn reexec_arguments_run_the_same_daemon_with_the_handoff() {
    let handoff: Handoff = "7:9".parse().unwrap();
    let unload_idle_after = Duration::from_millis(1500);

    let args = std::iter::once("rho".into()).chain(reexec_args(unload_idle_after, handoff));
    let parsed = Cli::try_parse_from(args).unwrap().command;

    let Some(Command::Daemon {
        command:
            Some(DaemonCommand::Run {
                unload_idle_after: parsed_idle,
                exec_handoff: Some(parsed_handoff),
            }),
    }) = parsed
    else {
        panic!("not `rho daemon run` with a handoff: {parsed:?}");
    };
    assert_eq!(
        (parsed_idle, parsed_handoff.parse::<Handoff>()),
        (Some(unload_idle_after), Ok(handoff))
    );
}
