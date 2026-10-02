//! Producer integration: actual workspace and process results remain typed in scripts.

use super::{script, CodeModeTool, ToolHost};
use crate::{
    config::Config,
    diagnostics::RuntimeDiagnostics,
    tools::sdk_registry::{AppToolSet, ToolSetOptions},
};
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn scripts_branch_on_workspace_and_process_results() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/a.rs"), "fn a() {}\n// TODO one\n").unwrap();
    std::fs::write(root.path().join("src/b.rs"), "// TODO two\n// TODO three\n").unwrap();
    let config = Config::default();
    let tools = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::default(),
    );
    let host = ToolHost::builder()
        .workspace(rho_sdk::Workspace::new(root.path()).unwrap())
        .workspace_policy(
            rho_sdk::ScopedWorkspacePolicy::new()
                .allow_read_paths()
                .allow_processes(),
        )
        .tool(CodeModeTool::new(tools.code_mode().clone()))
        .build()
        .unwrap();
    let output = script(&host, r#"
paths = call_tool("glob", {"pattern": "*.rs"})["data"]["paths"]
hits = call_tool("grep", {"pattern": "TODO"})["data"]
todo = {f["path"]: [l["line"] for l in f["lines"]] for f in hits["files"]}
kinds = {e["name"]: e["kind"] for e in call_tool("list_dir", {"path": "."})["data"]["entries"]}
failed = call_tool("bash", {"command": "printf oops >&2; exit 3"})
started = call_tool("process", {"action": "start", "command": "printf hi; exit 4"})["data"]
cursor = 0
out = ""
for _ in range(20):
    polled = call_tool("process", {"action": "poll", "process_id": started["process_id"], "cursor": cursor, "wait_seconds": 5})["data"]
    for chunk in polled["chunks"]:
        if chunk["stream"] == "stdout":
            out += chunk["text"]
    cursor = polled["next_cursor"]
    if polled["state"] not in ("running", "starting"):
        break
result = {
    "paths": sorted(paths), "todo": todo, "total": hits["total_matches"],
    "stopped": hits["stopped"], "kinds": kinds,
    "failure": [failed["data"]["exit_code"], failed["data"]["stderr"], failed["is_error"]] if failed["data"]["exit_code"] != 0 else None,
    "process": [polled["state"], polled["exit_code"], out, cursor > 0],
}
"#).await.unwrap();
    assert_eq!(
        output,
        json!({
            "paths": ["src/a.rs", "src/b.rs"], "todo": {"src/a.rs": [2], "src/b.rs": [1, 2]},
            "total": 3, "stopped": [], "kinds": {"src": "dir"}, "failure": [3, "oops", true],
            "process": ["exited", 4, "hi", true],
        })
    );
    tools.shutdown().await;
}
