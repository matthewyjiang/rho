//! Desktop consent and revocation through the real command path, with no desktop access.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

use anyhow::{ensure, Result};

use super::{SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
    PtyHarness,
};

// /new is idle-only. Seeing the response is not enough: the provider can still
// own the turn. Wait for its durable receipt, not an earlier turn's receipt.
fn wait_for_context_turn_completion(harness: &mut PtyHarness) -> Result<()> {
    let deadline = Instant::now() + STREAM.duration;
    loop {
        harness.poll(Duration::from_millis(25));
        let screen = harness.screen().contents();
        if screen
            .rfind("computer context: enabled")
            .is_some_and(|start| screen[start..].contains("Worked for"))
        {
            return Ok(());
        }
        ensure!(
            harness.is_running() && Instant::now() < deadline,
            "context turn did not finish before /new:\n{screen}"
        );
    }
}

pub(super) const COMPUTER_USE_SCENARIO: Scenario = Scenario::new(
    "computer_use",
    "Explicit desktop opt-in, connection, and revocation during a running turn",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "No desktop access granted",
            timeout: SETTLE,
        },
        Step::Custom(|harness| {
            if cfg!(target_os = "linux") {
                harness.wait_for_text("DISPLAY is unset or empty", SETTLE)?;
            }
            Ok(())
        }),
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "No desktop access granted",
            timeout: SETTLE,
        },
        Step::Resize { rows: 24, cols: 60 },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        // Security disclosure must be visible before consent, even when wrapped.
        Step::WaitText {
            text: "supervised mode",
            timeout: SETTLE,
        },
        Step::WaitText {
            text: "history.",
            timeout: SETTLE,
        },
        // Enter chooses the safe default, without starting a connection.
        Step::Key(Key::Enter),
        Step::WaitTextGone {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "No desktop access granted",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "No desktop access granted",
            timeout: SETTLE,
        },
        Step::SubmitText("/computer on"),
        Step::WaitText {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        // Resize the open consent overlay and wait for its clipped layout before
        // testing the hidden grant shortcut. Command submission must not race resize.
        Step::Resize { rows: 8, cols: 60 },
        Step::WaitTextGone {
            text: "history.",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('g')),
        Step::WaitText {
            text: "enlarge terminal",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::WaitTextGone {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        Step::Resize {
            rows: 40,
            cols: 120,
        },
        Step::SubmitText("/computer on"),
        Step::WaitText {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('g')),
        Step::WaitText {
            text: "computer use enabled",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "Desktop access granted for this session",
            timeout: SETTLE,
        },
        Step::Key(Key::Enter),
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: true",
            timeout: STREAM,
        },
        Step::SubmitText("fixture computer context"),
        Step::WaitText {
            text: "computer context: enabled",
            timeout: STREAM,
        },
        Step::Custom(wait_for_context_turn_completion),
        Step::SubmitText("/new"),
        Step::WaitTextGone {
            text: "computer context: enabled",
            timeout: SETTLE,
        },
        Step::WaitText {
            text: "computer use enabled",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture computer context"),
        Step::WaitText {
            text: "computer context: enabled",
            timeout: STREAM,
        },
        Step::WaitText {
            text: "computer use enabled",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture delay"),
        Step::WaitText {
            text: "partial assistant before cancellation",
            timeout: STREAM,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "computer use can only be enabled while the session is idle",
            timeout: SETTLE,
        },
        Step::SubmitText("/computer"),
        Step::WaitText {
            text: "Desktop access granted for this session",
            timeout: SETTLE,
        },
        // Revoke directly without dismissing the dashboard or interrupting.
        Step::Key(Key::Char('r')),
        Step::WaitText {
            text: "Access revoked; driver connection is closing",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        // Local notices wait for an open assistant message to finish. Input
        // order still applies off before Esc; assert the durable notice after
        // the stream boundary rather than racing the temporary status toast.
        Step::Key(Key::Esc),
        Step::WaitText {
            text: "computer use off; access revoked",
            timeout: SETTLE,
        },
        Step::WaitText {
            text: "model interrupted",
            timeout: STREAM,
        },
        // The same session's next request must not advertise the revoked tool.
        // /new would mask registry drift by rebuilding the entire session.
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: false",
            timeout: STREAM,
        },
        Step::SubmitText("fixture computer context"),
        Step::WaitText {
            text: "computer context: disabled",
            timeout: STREAM,
        },
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "No desktop access granted",
            timeout: SETTLE,
        },
        Step::Resize { rows: 18, cols: 60 },
        // Wait for the narrower viewport before sending a scroll key.
        Step::WaitTextGone {
            text: "/computer setup",
            timeout: SETTLE,
        },
        Step::Key(Key::End),
        Step::WaitText {
            text: "/computer setup",
            timeout: SETTLE,
        },
        Step::Key(Key::Home),
        Step::WaitText {
            text: "No desktop access granted",
            timeout: SETTLE,
        },
        Step::Key(Key::Esc),
        Step::SubmitText("fixture overlay closed"),
        Step::WaitText {
            text: "fixture response: fixture overlay closed",
            timeout: STREAM,
        },
        Step::ExitCommand,
        // Check the entire stream, including output a later redraw erased.
        Step::Custom(|harness| {
            ensure!(
                !harness
                    .raw_output()
                    .windows(b"fixture-cua-stderr".len())
                    .any(|window| window == b"fixture-cua-stderr"),
                "driver stderr leaked into the terminal"
            );
            Ok(())
        }),
    ],
    /*smoke*/ false,
)
.with_setup(setup_driver)
.with_env(&[
    ("PATH", ""),
    ("DISPLAY", ""),
    ("CUA_DRIVER_RS_TELEMETRY_ENABLED", "true"),
    ("CUA_TELEMETRY_ENABLED", "true"),
]);

pub(super) const COMPUTER_PLAN_SCENARIO: Scenario = Scenario::new(
    "computer_plan_denied",
    "Plan mode cannot grant desktop input authority",
    PtySize {
        rows: 30,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer setup"),
        Step::WaitText {
            text: "computer use is unavailable in plan mode",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_args(&["--permission-mode", "plan"]);

// Covers: a failed desktop connection must leave ordinary prompts usable.
// Owner: interactive command path; the runtime test fixes the completion race.
pub(super) const COMPUTER_FAILURE_SCENARIO: Scenario = Scenario::new(
    "computer_connect_failure",
    "A failed desktop connection does not prevent the next prompt",
    PtySize {
        rows: 40,
        cols: 120,
    },
    &[
        Step::WaitText {
            text: "gpt-5.5",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer on"),
        Step::WaitText {
            text: "Grant desktop access?",
            timeout: SETTLE,
        },
        Step::Key(Key::Char('g')),
        Step::WaitText {
            text: "could not connect computer use:",
            timeout: STARTUP,
        },
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: false",
            timeout: STREAM,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_failed_driver)
.with_env(&[("PATH", "")]);

fn setup_failed_driver(home: &IsolatedHome) -> Result<()> {
    let bin = home.home.join(".local/bin");
    fs::create_dir_all(&bin)?;
    let path = bin.join("cua-driver");
    fs::write(&path, "#!/bin/sh\nexit 1\n")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

pub(super) fn setup_driver(home: &IsolatedHome) -> Result<()> {
    let bin = home.home.join(".local/bin");
    fs::create_dir_all(&bin)?;
    let path = bin.join("cua-driver");
    // Only protocol discovery is implemented. Any accidental desktop tool call
    // fails, and PATH deliberately cannot select the developer's real driver.
    fs::write(
        &path,
        r##"#!/usr/bin/python3
import json, os, pathlib, sys
assert os.environ['CUA_DRIVER_RS_TELEMETRY_ENABLED'] == 'false'
assert os.environ['CUA_TELEMETRY_ENABLED'] == 'false'
home = pathlib.Path(__file__).parent.parent.parent
installed = (home / 'fixture-driver').exists()
if sys.argv[1:] == ['telemetry', 'disable']:
    # Existing-driver setup must never invoke the CLI before desktop consent.
    if not installed:
        (home / 'unexpected-telemetry-command').touch()
        sys.exit(1)
    (home / 'telemetry-disabled').write_text('false')
    sys.exit(0)
assert sys.argv[1:] == ['mcp']
assert not (home / 'unexpected-telemetry-command').exists()
if installed:
    assert (home / 'telemetry-disabled').read_text() == 'false'
print('fixture-cua-stderr startup notice', file=sys.stderr, flush=True)
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    if method == 'initialize':
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fake-cua', 'version': '1'}}
    elif method == 'tools/list':
        print('fixture-cua-stderr discovery warning', file=sys.stderr, flush=True)
        result = {'tools': [{'name': 'get_window_state', 'description': 'fixture observation', 'inputSchema': {'type': 'object'}}]}
    elif method == 'ping':
        result = {}
    else:
        print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'fixture has no desktop'}}), flush=True)
        continue
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"##,
    )?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}
