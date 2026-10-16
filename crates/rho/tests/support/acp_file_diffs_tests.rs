use pretty_assertions::assert_eq;
use serde_json::json;

use super::{AcpServer, Workspace};

// Covers: editors need full before/after file text, not a patch disguised as text,
// including every file in multi-file edits and the selected edit dialect.
// Owner: ACP wire contract, through the real server and coding tools.
#[test]
fn file_mutations_emit_acp_diffs() {
    let original = "alpha\r\nbeta\r\ngamma";
    // Exceed the existing 128 KiB unified-patch preview cap and the numbered
    // snapshot's head/tail window; neither may limit ACP's complete file text.
    let large = "row\n".repeat(128 * 1024 / "row\n".len());
    for (edit_tool, call, before, expected) in [
        (
            "hashline",
            json!({"name": "write", "arguments": {"path": "new.txt", "content": "created"}}),
            vec![],
            vec![json!({"type": "diff", "path": "new.txt", "newText": "created"})],
        ),
        (
            "hashline",
            json!({"name": "write", "arguments": {"path": "a.txt", "content": "rewritten\n"}}),
            vec![("a.txt", original.as_bytes())],
            vec![
                json!({"type": "diff", "path": "a.txt", "oldText": original, "newText": "rewritten\n"}),
            ],
        ),
        (
            "hashline",
            json!({"name": "edit", "arguments": {"input": "[a.txt#TAG]\nPUT 2.=2:\n+delta\n\n[b.txt#TAG]\nPUT 2.=2:\n+delta\n"}}),
            vec![
                ("a.txt", original.as_bytes()),
                ("b.txt", original.as_bytes()),
            ],
            vec![
                json!({"type": "diff", "path": "a.txt", "oldText": original, "newText": "alpha\r\ndelta\r\ngamma"}),
                json!({"type": "diff", "path": "b.txt", "oldText": original, "newText": "alpha\r\ndelta\r\ngamma"}),
            ],
        ),
        (
            "str_replace",
            json!({"name": "str_replace", "arguments": {"path": "a.txt", "old_string": "beta", "new_string": "delta"}}),
            vec![("a.txt", original.as_bytes())],
            vec![
                json!({"type": "diff", "path": "a.txt", "oldText": original, "newText": "alpha\r\ndelta\r\ngamma"}),
            ],
        ),
        (
            "apply_patch",
            json!({"name": "apply_patch", "arguments": {"input": "*** Begin Patch\n*** Update File: a.txt\n*** Move to: moved.txt\n@@\n-old\n+updated\n*** Delete File: deleted.txt\n*** Add File: added.txt\n+added\n*** Update File: changed.txt\n@@\n-old\n+changed\n*** End Patch"}}),
            vec![
                ("a.txt", b"old\n".as_slice()),
                ("deleted.txt", b"removed\n".as_slice()),
                ("changed.txt", b"old\n".as_slice()),
            ],
            vec![
                json!({"type": "diff", "path": "a.txt", "oldText": "old\n", "newText": ""}),
                json!({"type": "diff", "path": "moved.txt", "newText": "updated\n"}),
                json!({"type": "diff", "path": "deleted.txt", "oldText": "removed\n", "newText": ""}),
                json!({"type": "diff", "path": "added.txt", "newText": "added\n"}),
                json!({"type": "diff", "path": "changed.txt", "oldText": "old\n", "newText": "changed\n"}),
            ],
        ),
        (
            "hashline",
            json!({"name": "write", "arguments": {"path": "empty.txt", "content": ""}}),
            vec![],
            vec![json!({"type": "diff", "path": "empty.txt", "newText": ""})],
        ),
        (
            "hashline",
            json!({"name": "write", "arguments": {"path": "empty.txt", "content": "replacement"}}),
            vec![("empty.txt", b"".as_slice())],
            vec![
                json!({"type": "diff", "path": "empty.txt", "oldText": "", "newText": "replacement"}),
            ],
        ),
        (
            "hashline",
            json!({"name": "write", "arguments": {"path": "binary.bin", "content": "replacement"}}),
            vec![("binary.bin", b"\xff".as_slice())],
            vec![],
        ),
        (
            "hashline",
            json!({"name": "write", "arguments": {"path": "large.txt", "content": format!("updated\n{large}")}}),
            vec![("large.txt", large.as_bytes())],
            vec![
                json!({"type": "diff", "path": "large.txt", "oldText": large, "newText": format!("updated\n{large}")}),
            ],
        ),
    ] {
        let workspace = Workspace::new();
        let config = workspace.home.path().join("config.toml");
        let settings = std::fs::read_to_string(&config).unwrap().replace(
            "permission_mode = \"plan\"",
            &format!("permission_mode = \"bypass\"\nedit_tool = {edit_tool:?}"),
        );
        std::fs::write(config, settings).unwrap();
        for (path, text) in before {
            std::fs::write(workspace.project.join(path), text).unwrap();
        }
        let mut server = AcpServer::spawn(
            workspace.home.path(),
            &workspace.project,
            "tool-call",
            &workspace.project,
        );
        server
            .request("initialize", json!({"protocolVersion": 1}))
            .0
            .unwrap();
        let created = server
            .request(
                "session/new",
                json!({"cwd": workspace.project, "mcpServers": []}),
            )
            .0
            .unwrap();
        let mut call = call;
        call["id"] = json!("file-change");
        if call["name"] == "edit" {
            let snapshot = server.prompt(
                created["sessionId"].as_str().unwrap(),
                &json!({
                    "id": "read-snapshot", "name": "read_file", "arguments": {"path": "a.txt"},
                })
                .to_string(),
            );
            let tag = snapshot
                .lines()
                .next()
                .unwrap()
                .split_once('#')
                .unwrap()
                .1
                .trim_end_matches(']');
            call["arguments"]["input"] = json!(call["arguments"]["input"]
                .as_str()
                .unwrap()
                .replace("TAG", tag));
        }
        let (reply, updates) = server.request(
            "session/prompt",
            json!({
                "sessionId": created["sessionId"],
                "prompt": [{"type": "text", "text": call.to_string()}],
            }),
        );
        reply.unwrap();
        let finished = updates
            .iter()
            .find(|update| {
                update["sessionUpdate"] == "tool_call_update" && update["status"] == "completed"
            })
            .unwrap_or_else(|| panic!("tool did not complete: {updates:#?}"));
        let diffs: Vec<_> = finished["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|content| content["type"] == "diff")
            .cloned()
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|mut diff| {
                diff["path"] = json!(workspace.project.join(diff["path"].as_str().unwrap()));
                diff
            })
            .collect();
        assert_eq!(diffs, expected, "{edit_tool}: {}", call["name"]);
    }
}
