//! Desktop consent and revocation through the real command path, with no desktop access.

use std::{fs, os::unix::fs::PermissionsExt};

use anyhow::Result;

use super::{SETTLE, STARTUP, STREAM};
use crate::{
    env::IsolatedHome,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

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
            text: "Status: off",
            timeout: SETTLE,
        },
        Step::SubmitText("/computer on"),
        Step::WaitText {
            text: "the computer tool is available for the next turn",
            timeout: STARTUP,
        },
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "Status: connected",
            timeout: SETTLE,
        },
        Step::SubmitText("fixture tool available computer"),
        Step::WaitText {
            text: "tool available computer: true",
            timeout: STREAM,
        },
        Step::SubmitText("fixture delay"),
        Step::WaitText {
            text: "partial assistant before cancellation",
            timeout: STREAM,
        },
        Step::SubmitText("/computer off"),
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
        Step::SubmitText("/computer status"),
        Step::WaitText {
            text: "Status: off",
            timeout: SETTLE,
        },
        Step::ExitCommand,
    ],
    /*smoke*/ false,
)
.with_setup(setup_driver)
.with_env(&[("PATH", "")]);

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
        Step::SubmitText("/computer on"),
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

fn setup_driver(home: &IsolatedHome) -> Result<()> {
    let bin = home.home.join(".local/bin");
    fs::create_dir_all(&bin)?;
    let path = bin.join("cua-driver");
    // Only protocol discovery is implemented. Any accidental desktop tool call
    // fails, and PATH deliberately cannot select the developer's real driver.
    fs::write(
        &path,
        r##"#!/usr/bin/python3
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    if method == 'initialize':
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fake-cua', 'version': '1'}}
    elif method == 'tools/list':
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
