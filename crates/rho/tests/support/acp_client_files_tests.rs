//! Host-served files over a real `rho acp` stdio connection: a host that
//! advertises `fs.readTextFile` and `fs.writeTextFile` serves the text of
//! session files, unsaved edits included, and receives Rho's writes.

use std::{collections::BTreeMap, path::Path};

use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{AcpServer, Workspace, REPLY_TIMEOUT};

/// An editor host's open buffers, keyed by absolute path.
struct Buffers(BTreeMap<String, String>);

impl Buffers {
    /// Answers one `fs/*` request the way Zed does: from the buffer, falling
    /// back to disk for a file it has not opened yet, and failing for a file
    /// that is in neither. The answer is the reply's `result` or `error`.
    fn answer(&mut self, request: &Value) -> (&'static str, Value) {
        let params = &request["params"];
        let path = params["path"].as_str().unwrap().to_owned();
        match request["method"].as_str() {
            Some("fs/read_text_file") => {
                match self
                    .0
                    .get(&path)
                    .cloned()
                    .or_else(|| std::fs::read_to_string(&path).ok())
                {
                    Some(text) => ("result", json!({"content": text})),
                    None => (
                        "error",
                        json!({"code": -32002, "message": "Resource not found"}),
                    ),
                }
            }
            Some("fs/write_text_file") => {
                self.0
                    .insert(path, params["content"].as_str().unwrap().to_owned());
                ("result", json!({}))
            }
            _ => panic!("unexpected agent request: {request}"),
        }
    }
}

/// Prompts one tool call, answering the host requests it makes. Returns the
/// agent's reply text and each request as `[method, path, content]`.
fn prompt_through_host(
    server: &mut AcpServer,
    session_id: &str,
    call: Value,
    buffers: &mut Buffers,
    project: &Path,
) -> (String, Vec<Value>) {
    server.next_id += 1;
    let id = server.next_id;
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": call.to_string()}]},
    }));
    let (mut reply, mut requests) = (String::new(), Vec::new());
    loop {
        let message = server
            .messages
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the prompt progresses");
        match message["method"].as_str() {
            Some("session/update") => {
                let update = &message["params"]["update"];
                if update["sessionUpdate"] == "agent_message_chunk" {
                    reply.push_str(update["content"]["text"].as_str().unwrap_or_default());
                }
            }
            Some(method) => {
                assert_eq!(message["params"]["sessionId"], session_id, "{message}");
                let path = Path::new(message["params"]["path"].as_str().unwrap());
                requests.push(json!([
                    method,
                    path.strip_prefix(project).unwrap_or(path),
                    message["params"].get("content"),
                ]));
                let (kind, answer) = buffers.answer(&message);
                server.send(&json!({"jsonrpc": "2.0", "id": message["id"], kind: answer}));
            }
            None if message["id"] == id => {
                assert!(message.get("error").is_none(), "{message}");
                return (reply, requests);
            }
            None => panic!("reply to an unknown request: {message}"),
        }
    }
}

// Covers: with a host serving files, reads show the host's unsaved text,
// edits match against it and go back as `fs/write_text_file` while the disk
// copy stays untouched, results show the text the host saved, new files are
// created through the host, requests spell paths through the session cwd as
// the host gave it (here a symlink), and a launch `--add-dir` directory,
// which the host's project does not hold, is read from disk without asking,
// even when the host echoes it back.
// Owner: ACP client files, over the real stdio server and file tools
#[test]
fn host_served_files_route_reads_and_edits_through_the_host() {
    let workspace = Workspace::new();
    let config = workspace.home.path().join("config.toml");
    let settings = std::fs::read_to_string(&config).unwrap().replace(
        "permission_mode = \"plan\"",
        "permission_mode = \"bypass\"\nedit_tool = \"str_replace\"",
    );
    std::fs::write(config, settings).unwrap();
    let project = workspace.project.clone();
    let cwd = workspace.home.path().join("project-link");
    std::os::unix::fs::symlink(&project, &cwd).unwrap();
    let tracked_on_host = cwd.join("a.txt");
    let tracked = project.join("a.txt");
    std::fs::write(&tracked, "alpha\nbeta\n").unwrap();
    let mut buffers = Buffers(BTreeMap::from([(
        tracked_on_host.to_str().unwrap().to_owned(),
        "alpha\nunsaved\n".to_owned(),
    )]));

    let mut server = AcpServer::from_command(
        AcpServer::command(workspace.home.path(), &project, "tool-call", &project)
            .arg("--add-dir")
            .arg(&workspace.extra),
    );
    server
        .request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}},
            }),
        )
        .0
        .unwrap();
    let created = server
        .request(
            "session/new",
            // A host echoing the launch directory, as `session/list` reports it.
            json!({"cwd": cwd, "mcpServers": [], "additionalDirectories": [workspace.extra]}),
        )
        .0
        .unwrap();
    let session_id = created["sessionId"].as_str().unwrap().to_owned();

    let outside = workspace.extra.join("notes.txt");
    let mut asked = Vec::new();
    let mut replies = Vec::new();
    for call in [
        json!({"name": "read_file", "arguments": {"path": "a.txt"}}),
        json!({"name": "str_replace", "arguments": {"path": "a.txt", "old_string": "unsaved", "new_string": "edited"}}),
        json!({"name": "write", "arguments": {"path": "new.txt", "content": "created\n"}}),
        json!({"name": "read_file", "arguments": {"path": outside}}),
    ] {
        let mut call = call;
        call["id"] = json!("host-files");
        let (reply, requests) =
            prompt_through_host(&mut server, &session_id, call, &mut buffers, &cwd);
        replies.push(reply.lines().last().unwrap_or_default().to_owned());
        asked.push(requests);
    }

    let read = json!(["fs/read_text_file", "a.txt", null]);
    assert_eq!(
        (asked, replies),
        (
            vec![
                vec![read.clone()],
                // The match, a fresh read just before writing, and a read
                // of what the host saved.
                vec![
                    read.clone(),
                    read.clone(),
                    json!(["fs/write_text_file", "a.txt", "alpha\nedited\n"]),
                    read,
                ],
                // Whether the host holds it unsaved, then the write and the
                // read of what the host saved.
                vec![
                    json!(["fs/read_text_file", "new.txt", null]),
                    json!(["fs/write_text_file", "new.txt", "created\n"]),
                    json!(["fs/read_text_file", "new.txt", null]),
                ],
                vec![],
            ],
            vec![
                "2:unsaved".to_owned(),
                "2:edited".to_owned(),
                "1:created".to_owned(),
                "1:extra root content".to_owned(),
            ],
        )
    );
    assert_eq!(
        (
            std::fs::read_to_string(&tracked).unwrap(),
            project.join("new.txt").exists()
        ),
        ("alpha\nbeta\n".to_owned(), false)
    );
}
