//! Covers unsolicited parent inference from ordinary child notices.
//! Owner: interactive turn-boundary scheduling, through real delegated tools.

use std::{
    collections::HashSet,
    os::unix::net::UnixDatagram,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use super::{DEFAULT_SIZE, STARTUP, STREAM};
use crate::{
    scenario::{Scenario, Step},
    PtyHarness,
};

fn quiet_notices_then_action(harness: &mut PtyHarness) -> Result<()> {
    let cwd = harness
        .working_directory()
        .context("scenario working directory")?
        .to_path_buf();
    let socket = UnixDatagram::bind(cwd.join(".quiet-parent-pty.sock"))?;
    socket.set_nonblocking(true)?;
    harness.submit_text("fixture quiet subagent")?;
    harness.wait_for_text("quiet child dispatched", STREAM)?;

    let mut received = HashSet::new();
    for (stage, expected) in [("first", "boundary:1"), ("second", "boundary:2")] {
        wait_signal(harness, &socket, &mut received, stage)?;
        socket.send_to(b"!", cwd.join(format!(".quiet-child-{stage}.sock")))?;
        // These acknowledgments come from the parent's notification boundary.
        // A child tool receipt alone would race the parent handling its inbox.
        wait_signal(harness, &socket, &mut received, expected)?;
    }
    wait_signal(harness, &socket, &mut received, "action")?;
    socket.send_to(b"!", cwd.join(".quiet-child-action.sock"))?;
    // The live child never completes. Only request_parent_action can wake this
    // turn; both earlier notices must arrive exactly once in that same request.
    harness.wait_for_text("quiet delivery requests=1 occurrences=[1, 1, 1]", STREAM)?;
    Ok(())
}

fn wait_signal(
    harness: &mut PtyHarness,
    socket: &UnixDatagram,
    received: &mut HashSet<String>,
    expected: &str,
) -> Result<()> {
    let deadline = Instant::now() + STREAM.duration;
    // `boundary:<usize>` is the longest packet; stage names are shorter.
    let mut packet = vec![0_u8; format!("boundary:{}", usize::MAX).len()];
    loop {
        if received.remove(expected) {
            return Ok(());
        }
        match socket.recv(&mut packet) {
            Ok(length) => {
                received.insert(std::str::from_utf8(&packet[..length])?.to_owned());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                harness.poll(Duration::from_millis(20));
            }
            Err(error) => return Err(error.into()),
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "quiet child barrier did not receive {expected}; received {received:?}\n{}",
            harness.screen().debug_dump()
        );
    }
}

// Covers a consumed action request stranding the failed parent turn behind the
// still-running child. Ordinary goal retries retain their completion wait.
fn goal_action_retry(harness: &mut PtyHarness) -> Result<()> {
    let cwd = harness
        .working_directory()
        .context("scenario working directory")?
        .to_path_buf();
    let socket = UnixDatagram::bind(cwd.join(".quiet-parent-pty.sock"))?;
    socket.set_nonblocking(true)?;
    harness.submit_text("/goal fixture quiet action retry")?;
    harness.wait_for_text("quiet child dispatched", STREAM)?;
    wait_signal(harness, &socket, &mut HashSet::new(), "action")?;
    socket.send_to(b"!", cwd.join(".quiet-child-action.sock"))?;
    harness.wait_for_text("quiet delivery requests=2 occurrences=[0, 0, 1]", STREAM)?;
    harness.submit_text("/goal clear")?;
    harness.wait_for_text("goal cleared", STREAM)?;
    Ok(())
}

fn running_notices(harness: &mut PtyHarness) -> Result<()> {
    let cwd = harness
        .working_directory()
        .context("scenario working directory")?
        .to_path_buf();
    let socket = UnixDatagram::bind(cwd.join(".quiet-parent-pty.sock"))?;
    socket.set_nonblocking(true)?;
    let mut received = HashSet::new();
    harness.submit_text("fixture quiet running subagent")?;
    wait_signal(harness, &socket, &mut received, "parent")?;
    wait_signal(harness, &socket, &mut received, "first")?;
    socket.send_to(b"!", cwd.join(".quiet-child-first.sock"))?;
    wait_signal(harness, &socket, &mut received, "posted")?;
    socket.send_to(b"!", cwd.join(".quiet-child-parent.sock"))?;
    harness.wait_for_text("quiet running parent completed", STREAM)?;
    harness.submit_text("fixture quiet request count")?;
    harness.wait_for_text("quiet extra requests=0 carried notices=1", STREAM)?;
    Ok(())
}

pub(super) const RUNNING_NOTICES_SCENARIO: Scenario = Scenario::new(
    "quiet_subagent_running_notices",
    "Do not buy another provider request for a notice at the completion checkpoint",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(running_notices),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

pub(super) const QUIET_SUBAGENT_SCENARIO: Scenario = Scenario::new(
    "quiet_subagent_notices",
    "Keep ordinary child notices queued and coalesce them into the requested parent turn",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(quiet_notices_then_action),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

pub(super) const GOAL_ACTION_RETRY_SCENARIO: Scenario = Scenario::new(
    "goal_subagent_action_retry",
    "Retry failed parent action without waiting for the blocked child to finish",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(goal_action_retry),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);
