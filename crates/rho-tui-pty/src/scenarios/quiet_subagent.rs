//! Covers unsolicited parent inference from ordinary child notices.
//! Also covers incoming child message bodies being lost from the transcript.
//! Owner: interactive turn-boundary delivery, through real delegated tools.

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
    // Provider delivery alone does not prove the user can read the messages.
    // These are the child tool payloads, not card titles or the parent's reply.
    for message in [
        "quiet-cache-inspected",
        "quiet-routing-inspected",
        "quiet-decision-required",
    ] {
        harness.wait_for_text(message, STREAM)?;
    }
    harness.inject_key(&crate::keys::Key::Ctrl('o'))?;
    harness.wait_for_text("task: ", STREAM)?;
    harness.inject_key(&crate::keys::Key::Ctrl('o'))?;
    harness.wait_for_text_gone("task: ", STREAM)?;
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
    resume_and_check_message(harness, "quiet-decision-required")?;
    Ok(())
}

// The accepted failed turn owns the receipt. A retry must not duplicate it on
// resume, and folding a notice into a human prompt must not discard it.
fn resume_and_check_message(harness: &mut PtyHarness, message: &str) -> Result<()> {
    harness.submit_text("/new")?;
    harness.wait_for_text("new session", STREAM)?;
    harness.submit_text("/resume")?;
    harness.wait_for_text("Resume session", STREAM)?;
    harness.inject_key(&crate::keys::Key::Enter)?;
    harness.wait_for_text(message, STREAM)?;
    let rows = harness.screen().rows_text();
    let message_rows = rows
        .iter()
        .filter(|row| row.contains(message))
        .collect::<Vec<_>>();
    anyhow::ensure!(
        message_rows.len() == 1 && message_rows[0].trim_start().starts_with('│'),
        "resumed message must appear once as an incoming card:\n{}",
        harness.screen().contents(),
    );
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
    resume_and_check_message(harness, "quiet-cache-inspected")?;
    Ok(())
}

// Covers: an accepted boundary notice must not split already rendered text
// from queued provider deltas or the typewriter's held suffix.
fn streaming_notice(harness: &mut PtyHarness) -> Result<()> {
    let cwd = harness
        .working_directory()
        .context("scenario working directory")?
        .to_path_buf();
    let socket = UnixDatagram::bind(cwd.join(".quiet-parent-pty.sock"))?;
    socket.set_nonblocking(true)?;
    let mut received = HashSet::new();
    harness.submit_text("fixture streaming notice")?;
    wait_signal(harness, &socket, &mut received, "parent")?;
    harness.wait_for_text("The checks ha", STREAM)?;
    wait_signal(harness, &socket, &mut received, "first")?;
    socket.send_to(b"!", cwd.join(".quiet-child-first.sock"))?;
    wait_signal(harness, &socket, &mut received, "posted")?;
    socket.send_to(b"!", cwd.join(".quiet-child-parent.sock"))?;
    harness.wait_for_text("quiet delivery requests=1 occurrences=[1, 0, 0]", STREAM)?;
    let screen = harness.screen().contents();
    let start = screen
        .find("Streaming verification results:")
        .context("stream prefix")?;
    let end = screen
        .find("The checks have passed without interruption.")
        .context("intact stream suffix")?;
    let notice = screen
        .find("quiet-cache-inspected")
        .context("delivered child message body")?;
    let follow_up = screen
        .find("quiet delivery requests=1 occurrences=[1, 0, 0]")
        .context("parent incorporated the notice")?;
    anyhow::ensure!(
        start < end && end < notice && notice < follow_up,
        "notification split or overtook the assistant message:\n{screen}"
    );
    Ok(())
}

pub(super) const STREAMING_NOTICE_SCENARIO: Scenario = Scenario::new(
    "streaming_boundary_notice",
    "Keep a streaming assistant message intact when a boundary notice arrives",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(streaming_notice),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

// Covers: /new must attribute new children to the replacement session, so an
// idle parent receives their final result without another user prompt.
fn completion_after_new(harness: &mut PtyHarness) -> Result<()> {
    let cwd = harness
        .working_directory()
        .context("scenario working directory")?
        .to_path_buf();
    let socket = UnixDatagram::bind(cwd.join(".quiet-parent-pty.sock"))?;
    socket.set_nonblocking(true)?;
    harness.submit_text("fixture initial session")?;
    harness.wait_for_text("fixture response: fixture initial session", STREAM)?;
    harness.submit_text("/new")?;
    harness.wait_for_text("new session", STREAM)?;
    // This first prompt attaches storage for the new session before spawning.
    harness.submit_text("fixture quiet completion subagent")?;
    harness.wait_for_text("quiet completion child dispatched", STREAM)?;
    // The completion footer proves the parent turn ended before child release.
    harness.wait_for_text("Worked for", STREAM)?;
    wait_signal(harness, &socket, &mut HashSet::new(), "completion")?;
    socket.send_to(b"!", cwd.join(".quiet-child-completion.sock"))?;
    harness.wait_for_text("quiet final delivery requests=1 occurrences=1", STREAM)?;
    harness.submit_text("fixture quiet completion count")?;
    harness.wait_for_text("quiet completion requests=1 carried finals=0", STREAM)?;
    Ok(())
}

pub(super) const COMPLETION_AFTER_NEW_SCENARIO: Scenario = Scenario::new(
    "subagent_completion_after_new",
    "Deliver a new session's child completion automatically and exactly once after /new",
    DEFAULT_SIZE,
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::Custom(completion_after_new),
        Step::ExitCommand,
    ],
    /*smoke*/ true,
);

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
    "Queue child notices, coalesce their delivery, and show their bodies in the transcript",
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
