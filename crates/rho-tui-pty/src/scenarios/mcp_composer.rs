//! What an MCP server offers the composer: a resource picked from the `@`
//! palette arrives as an attachment, and a prompt command's argument
//! completes from the server's suggestions. The scenario runs twice, with
//! the session in process and with the daemon holding it, since the servers
//! are the session's either way.

use std::fs;

use anyhow::{Context, Result};

use crate::{
    env::IsolatedHome,
    harness::PtyHarness,
    keys::Key,
    pty::PtySize,
    scenario::{Scenario, Step},
};

use super::{SETTLE, STARTUP, STREAM};

/// A stdio MCP server with one text resource and one prompt whose argument
/// it completes by prefix. It speaks only what connecting and the composer
/// need.
const SERVER: &str = r#"
import json, sys

VALUES = ["alice-from-server", "alfred-from-server", "bob-from-server"]

def answer(method, params):
    if method == "initialize":
        return {
            "protocolVersion": params["protocolVersion"],
            "capabilities": {"tools": {}, "prompts": {}, "resources": {}, "completions": {}},
            "serverInfo": {"name": "composer-fixture", "version": "1"},
        }
    if method == "tools/list":
        return {"tools": []}
    if method == "prompts/list":
        return {"prompts": [{"name": "greet", "arguments": [{"name": "who", "required": True}]}]}
    if method == "prompts/get":
        who = params["arguments"]["who"]
        return {"messages": [{
            "role": "user",
            "content": {"type": "text", "text": "say hello to " + who},
        }]}
    if method == "resources/list":
        return {"resources": [{"uri": "notes://fixture/release", "name": "release"}]}
    if method == "resources/templates/list":
        return {"resourceTemplates": []}
    if method == "resources/read":
        return {"contents": [{
            "uri": params["uri"],
            "mimeType": "text/plain",
            "text": "release notes from the server",
        }]}
    if method == "completion/complete":
        typed = params["argument"]["value"]
        return {"completion": {"values": [v for v in VALUES if v.startswith(typed)]}}
    if method == "ping":
        return {}
    return None

for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message or "method" not in message:
        continue
    reply = {"jsonrpc": "2.0", "id": message["id"]}
    result = answer(message["method"], message.get("params") or {})
    if result is None:
        reply["error"] = {"code": -32601, "message": "method not found"}
    else:
        reply["result"] = result
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()
"#;

fn setup_composer_server(home: &IsolatedHome) -> Result<()> {
    let server = home.home.join("mcp-composer-fixture.py");
    fs::write(&server, SERVER).context("write the MCP fixture server")?;
    let existing = fs::read_to_string(&home.config_path).context("read isolated config")?;
    fs::write(
        &home.config_path,
        format!(
            "{existing}\n[mcp.servers.fixture]\ntransport = \"stdio\"\ncommand = \"python3\"\nargs = [{:?}]\n",
            server.display().to_string()
        ),
    )
    .context("append the MCP fixture server to isolated config")?;
    Ok(())
}

fn accept_highlighted_row(harness: &mut PtyHarness) -> Result<()> {
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Enter)
}

fn complete_highlighted_row(harness: &mut PtyHarness) -> Result<()> {
    harness.settle_plain_text_input();
    harness.inject_key(&Key::Tab)
}

// Covers: the composer reads resources and prompt-argument suggestions from
// the session's MCP servers. A host that cannot reach them lists no resource
// under `@` and suggests nothing for a prompt command's argument.
// Owner: interactive TUI
const MCP_COMPOSER_STEPS: &[Step] = &[
    Step::Phase("startup"),
    Step::WaitText {
        text: "gpt-5.5",
        timeout: STARTUP,
    },
    // Typed while the server may still connect: the row appears once it has.
    Step::Phase("mention_resource"),
    Step::TypeText("@release"),
    Step::WaitText {
        text: "notes://fixture/release",
        timeout: SETTLE,
    },
    Step::Custom(accept_highlighted_row),
    // The attachment holds the 29 characters the server sent, and they
    // reach the model, so the turn's echo carries them.
    Step::Phase("send_resource"),
    Step::WaitText {
        text: "notes://fixture/release · 29 chars]",
        timeout: SETTLE,
    },
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "release notes from the server",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Type a message",
        timeout: STREAM,
    },
    Step::Phase("complete_prompt_argument"),
    Step::TypeText("/mcp:fixture:greet al"),
    Step::WaitText {
        text: "alfred-from-server",
        timeout: SETTLE,
    },
    Step::AssertText("alice-from-server"),
    Step::Custom(complete_highlighted_row),
    Step::WaitText {
        text: "/mcp:fixture:greet alice-from-server",
        timeout: SETTLE,
    },
    // The prompt expands with the completed value, which the turn echoes.
    Step::Phase("run_prompt"),
    Step::Key(Key::Enter),
    Step::WaitText {
        text: "say hello to alice-from-server",
        timeout: STREAM,
    },
    Step::WaitText {
        text: "Type a message",
        timeout: STREAM,
    },
    Step::ExitCommand,
];

pub(super) const MCP_COMPOSER_SCENARIO: Scenario = Scenario::new(
    "mcp_composer",
    "Attach an MCP resource from @ and complete an MCP prompt argument",
    PtySize {
        rows: 30,
        cols: 120,
    },
    MCP_COMPOSER_STEPS,
    /* smoke */ false,
)
.with_setup(setup_composer_server);

pub(super) const DAEMON_MCP_COMPOSER_SCENARIO: Scenario = Scenario {
    id: "daemon_mcp_composer",
    description: "Attach an MCP resource and complete a prompt argument in a daemon session",
    env: &[("RHO_TUI_DAEMON", "1")],
    ..MCP_COMPOSER_SCENARIO
};
