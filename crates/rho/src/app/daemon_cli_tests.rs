use std::time::Duration;

use clap::Parser;
use pretty_assertions::assert_eq;

use super::rerun_args;
use crate::cli::{Cli, Command, DaemonCommand};

// Covers: the arguments a daemon runs itself again with parse back to the
// same `rho daemon run`, carrying its idle period and the handoff token, so
// a renamed option or a new one cannot silently drop out of the restart.
// Owner: daemon restart in place.
#[test]
fn rerun_arguments_run_the_same_daemon_with_the_handoff() {
    let handoff = "7:9".to_owned();
    let unload_idle_after = Duration::from_millis(1500);

    let args =
        std::iter::once("rho".into()).chain(rerun_args(unload_idle_after, Some(handoff.clone())));
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
        (parsed_idle, parsed_handoff),
        (Some(unload_idle_after), handoff)
    );
}
