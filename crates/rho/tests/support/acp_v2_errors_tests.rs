//! The errors ACP v2 names for requests it forbids, the negotiation of
//! `initialize`, and the host hanging up first.

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace, REPLY_TIMEOUT};

fn initialize_v2(capabilities: Value) -> Value {
    json!({
        "protocolVersion": 2,
        "info": {"name": "errors-test", "version": "1.0.0"},
        "capabilities": capabilities,
    })
}

/// A v2 connection with one new session in the workspace.
fn v2_session(workspace: &Workspace) -> (AcpServer, String) {
    let mut server = workspace.server();
    server
        .request("initialize", initialize_v2(json!({})))
        .0
        .unwrap();
    let session = server
        .request("session/new", json!({"cwd": workspace.project}))
        .0
        .unwrap();
    (server, session["sessionId"].as_str().unwrap().to_owned())
}

// Covers: each request v2 forbids gets the code v2 names, with
// `data.sessionId` for an unknown session; a refused prompt sends no update.
// Owner: ACP v2 request validation, over the real stdio server
#[test]
fn v2_refuses_forbidden_requests_with_the_codes_it_names() {
    let workspace = Workspace::new();
    let (mut server, session) = v2_session(&workspace);
    let cwd = workspace.project.clone();
    let text = json!({"type": "text", "text": "hi"});
    let stdio = |name: &str, command: &str| json!({"type": "stdio", "name": name, "command": command, "args": [], "env": []});
    let cases = [
        (
            "prompt in a never-created session",
            "session/prompt",
            json!({"sessionId": "never-created", "prompt": [text]}),
            json!({"code": -32002, "sessionId": "never-created"}),
        ),
        (
            "set_config_option in a never-created session",
            "session/set_config_option",
            json!({"sessionId": "never-created", "configId": "mode", "type": "id", "value": "plan"}),
            json!({"code": -32002, "sessionId": "never-created"}),
        ),
        (
            "tool_call/cancel in a never-created session",
            "tool_call/cancel",
            json!({"sessionId": "never-created", "toolCallId": "t"}),
            json!({"code": -32002, "sessionId": "never-created"}),
        ),
        (
            "resume of a never-saved session",
            "session/resume",
            json!({"sessionId": "never-saved", "cwd": cwd}),
            json!({"code": -32002, "sessionId": "never-saved"}),
        ),
        (
            "audio, which Rho does not advertise",
            "session/prompt",
            json!({"sessionId": session, "prompt": [
                text,
                {"type": "audio", "data": "AAAA", "mimeType": "audio/wav"},
            ]}),
            json!({"code": -32602, "sessionId": null}),
        ),
        (
            "an empty prompt",
            "session/prompt",
            json!({"sessionId": session, "prompt": []}),
            json!({"code": -32602, "sessionId": null}),
        ),
        (
            "two MCP servers with one name",
            "session/new",
            json!({"cwd": cwd, "mcpServers": [
                stdio("twin", "/bin/true"),
                {"type": "http", "name": "twin", "url": "http://127.0.0.1:9/mcp", "headers": []},
            ]}),
            json!({"code": -32602, "sessionId": null}),
        ),
        (
            "a relative stdio MCP command",
            "session/new",
            json!({"cwd": cwd, "mcpServers": [stdio("relative", "bin/server")]}),
            json!({"code": -32602, "sessionId": null}),
        ),
    ];
    for (case, method, params, expected) in cases {
        let (reply, updates) = server.request(method, params);
        let error = reply.expect_err(case);
        assert_eq!(
            json!({"code": error["code"], "sessionId": error["data"]["sessionId"]}),
            expected,
            "{case}: {error}"
        );
        assert_eq!(updates, Vec::<Value>::new(), "{case} sent updates");
    }
}

// Covers: `protocolVersion: 0` is refused with `-32022` instead of falling
// back to v1, and a failed `initialize` leaves the connection open to a
// corrected one, which then gets v2.
// Owner: `rho acp` stdio transport, over the real stdio server
#[test]
fn initialize_negotiates_v2_after_a_refused_version() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    let (zero, _) = server.request(
        "initialize",
        json!({"protocolVersion": 0, "info": {"name": "h", "version": "1"}}),
    );
    assert_eq!(
        zero,
        Err(json!({
            "code": -32022,
            "message": "Unsupported protocol version",
            "data": {"requested": 0, "supported": [1, 2]},
        }))
    );
    let (missing, _) = server.request(
        "initialize",
        json!({"info": {"name": "h", "version": "1"}, "capabilities": {}}),
    );
    assert_eq!(missing.unwrap_err()["code"], -32602);
    let (corrected, _) = server.request("initialize", initialize_v2(json!({})));
    assert_eq!(corrected.unwrap()["protocolVersion"], 2);
}

// Covers: a repeated v2 `initialize` replaces what the first negotiated;
// here the second takes back URL elicitation, so sign-in is refused before
// it starts instead of showing a link.
// Owner: ACP v2 initialize handler, over the real stdio server
#[test]
fn the_latest_v2_initialize_decides_what_the_host_supports() {
    let workspace = Workspace::new();
    let mut server = workspace.server();
    for capabilities in [json!({"elicitation": {"url": {}}}), json!({})] {
        server
            .request("initialize", initialize_v2(capabilities))
            .0
            .unwrap();
    }
    let (login, updates) = server.request("auth/login", json!({"methodId": "xai-oauth"}));
    assert_eq!(login.unwrap_err()["code"], -32602);
    assert_eq!(updates, Vec::<Value>::new());
}

// Covers: a host that closes the connection without reading the last reply
// makes `rho acp` exit cleanly, not fail on the broken pipe.
// Owner: `rho acp` stdio transport, over the real process
#[test]
fn rho_acp_exits_cleanly_when_the_host_hangs_up_first() {
    let workspace = Workspace::new();
    let mut child = AcpServer::command(&workspace.home, "read-path", &workspace.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rho acp");
    // Read as it arrives, never joined: the daemon `rho acp` starts may
    // hold the pipe open after `rho acp` exits.
    let stderr = Arc::new(Mutex::new(String::new()));
    std::thread::spawn({
        let mut agent_stderr = child.stderr.take().unwrap();
        let stderr = Arc::clone(&stderr);
        move || {
            let mut chunk = [0; 4096];
            while let Ok(read @ 1..) = agent_stderr.read(&mut chunk) {
                stderr
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&chunk[..read]));
            }
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    // The host's reader: it stops reading, and closes its end, once
    // `session/new` is answered.
    let (replies, reply) = mpsc::channel::<Value>();
    let reader = std::thread::spawn({
        let stdout = BufReader::new(child.stdout.take().unwrap());
        move || {
            for line in stdout.lines() {
                let message: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let last = message["id"] == 2 && message.get("method").is_none();
                if replies.send(message).is_err() || last {
                    return;
                }
            }
        }
    });
    let session = {
        let mut exchange = |id: u64, method: &str, params: Value| {
            let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            writeln!(stdin, "{request}").unwrap();
            loop {
                let message = reply
                    .recv_timeout(REPLY_TIMEOUT)
                    .unwrap_or_else(|error| panic!("no reply to {method}: {error}"));
                if message["id"] == id && message.get("method").is_none() {
                    return message["result"].clone();
                }
            }
        };
        exchange(1, "initialize", initialize_v2(json!({})));
        exchange(2, "session/new", json!({"cwd": workspace.project}))
    };
    reader.join().unwrap();
    // The host no longer reads, then asks for a reply it will never read.
    let close = json!({"jsonrpc": "2.0", "id": 3, "method": "session/close",
        "params": {"sessionId": session["sessionId"]}});
    writeln!(stdin, "{close}").unwrap();

    let pid = child.id();
    let (exited, status) = mpsc::channel();
    std::thread::spawn(move || exited.send(child.wait()));
    let status = status.recv_timeout(REPLY_TIMEOUT);
    if status.is_err() {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
    drop(stdin);
    let stderr = stderr.lock().unwrap().clone();
    let status = status
        .unwrap_or_else(|_| {
            panic!("rho acp did not exit after the host hung up\nstderr:\n{stderr}")
        })
        .unwrap();
    assert!(
        status.success(),
        "rho acp exited with {status}\nstderr:\n{stderr}"
    );
}
