use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use rho_sdk::tool::{Tool, ToolOutput};
use serde_json::{json, Value};

use super::bridge::{BridgeError, CodeModeBridge, GuardedBridge, CODEMODE_TOOL_NAME};
use super::engine::{evaluate_code_mode, format_engine_output, EngineLimits, EngineOutput};
use super::tool::CodeModeTool;

struct StubBridge {
    calls: Mutex<Vec<(String, Value)>>,
    responses: BTreeMap<String, String>,
}

#[async_trait]
impl CodeModeBridge for StubBridge {
    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, BridgeError> {
        self.calls
            .lock()
            .expect("calls")
            .push((name.to_owned(), arguments));
        let body = self
            .responses
            .get(name)
            .cloned()
            .unwrap_or_else(|| format!("ok:{name}"));
        Ok(ToolOutput::text(body))
    }
}

fn guarded(
    responses: BTreeMap<String, String>,
    allowlist: Option<BTreeSet<String>>,
) -> Arc<GuardedBridge> {
    Arc::new(GuardedBridge::new(
        Arc::new(StubBridge {
            calls: Mutex::new(Vec::new()),
            responses,
        }),
        allowlist,
        32,
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn script_can_call_native_and_mcp_named_tools() {
    let mut responses = BTreeMap::new();
    responses.insert("read_file".into(), "file-bytes".into());
    responses.insert("mcp_demo__search".into(), r#"{"hits":2}"#.into());
    let allow = BTreeSet::from([
        "read_file".to_owned(),
        "mcp_demo__search".to_owned(),
    ]);
    let bridge = guarded(responses, Some(allow));
    let script = r#"
native = call_tool("read_file", {"path": "README.md"})
mcp = call_tool("mcp_demo__search", {"q": "rho"})
print(native["content"])
result = {"native": native["content"], "mcp": mcp["content"]}
"#;
    let output = tokio::task::block_in_place(|| {
        evaluate_code_mode(script, Arc::clone(&bridge), EngineLimits::default())
    })
    .expect("evaluate");
    assert_eq!(output.nested_calls, 2);
    assert_eq!(output.prints, vec!["file-bytes".to_owned()]);
    assert_eq!(output.return_value["native"], json!("file-bytes"));
    assert_eq!(output.return_value["mcp"], json!(r#"{"hits":2}"#));
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_tools_not_on_allowlist_loudly() {
    let bridge = guarded(BTreeMap::new(), Some(BTreeSet::from(["read_file".into()])));
    let err = tokio::task::block_in_place(|| {
        evaluate_code_mode(
            r#"result = call_tool("bash", {"command": "echo hi"})"#,
            bridge,
            EngineLimits::default(),
        )
    })
    .expect_err("must deny");
    let message = err.to_string();
    assert!(
        message.contains("not on the allowlist"),
        "unexpected error: {message}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refuse_recursive_codemode() {
    let bridge = guarded(BTreeMap::new(), None);
    let err = tokio::task::block_in_place(|| {
        evaluate_code_mode(
            &format!(r#"result = call_tool("{CODEMODE_TOOL_NAME}", {{"script": "1"}})"#),
            bridge,
            EngineLimits::default(),
        )
    })
    .expect_err("must refuse recursion");
    assert!(err.to_string().contains("refusing recursive"));
}

#[test]
fn tool_spec_and_format_smoke() {
    let bridge = guarded(BTreeMap::new(), None);
    let tool = CodeModeTool::with_bridge(bridge, EngineLimits::default());
    assert_eq!(tool.spec().name, CODEMODE_TOOL_NAME);
    let formatted = format_engine_output(&EngineOutput {
        return_value: json!("pong"),
        prints: vec![],
        nested_calls: 1,
    });
    assert!(formatted.contains("pong"), "{formatted}");
}
