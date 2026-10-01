use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use rho_sdk::tool::ToolOutput;
use serde_json::{json, Value};

use super::bridge::{BridgeError, CodeModeBridge, GuardedBridge, CODEMODE_TOOL_NAME};
use super::engine::{evaluate_code_mode, format_engine_output, EngineLimits, EngineOutput};

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
    let allow = BTreeSet::from(["read_file".to_owned(), "mcp_demo__search".to_owned()]);
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

// Covers: scripts may use top-level `for`/`if` (models write glue top-level),
// and the tick limit still stops a top-level loop that runs too long.
// Owner: codemode Starlark dialect + engine limits.
#[tokio::test(flavor = "multi_thread")]
async fn top_level_control_flow_runs_within_limits() {
    let output = evaluate_code_mode(
        r#"
seen = []
for name in ["a", "b", "c"]:
    if name != "b":
        seen.append(call_tool("read_file", {"path": name})["content"])
result = seen
"#,
        guarded(BTreeMap::new(), None),
        EngineLimits::default(),
    )
    .unwrap();
    assert_eq!(
        (output.return_value, output.nested_calls),
        (json!(["ok:read_file", "ok:read_file"]), 2)
    );

    let runaway = evaluate_code_mode(
        "total = 0\nfor i in range(10000000):\n    total += i\n",
        guarded(BTreeMap::new(), None),
        EngineLimits::default(),
    );
    assert!(runaway.is_err(), "tick limit must stop a top-level loop");
}

#[test]
fn format_engine_output_includes_return_value() {
    let formatted = format_engine_output(&EngineOutput {
        return_value: json!("pong"),
        prints: vec![],
        nested_calls: 1,
    });
    assert!(formatted.contains("pong"), "{formatted}");
}

#[tokio::test(flavor = "multi_thread")]
async fn script_search_tools_finds_mcp_and_call_tool() {
    use super::engine::evaluate_code_mode_with_exposure;
    use super::exposure::ExposureController;

    let mut responses = BTreeMap::new();
    responses.insert("mcp__github__create_issue".into(), "created".into());
    let allow = BTreeSet::from(["mcp__github__create_issue".to_owned()]);
    let exposure = Arc::new(ExposureController::with_default_policy());
    exposure.index_tool("mcp__github__create_issue", "Create a GitHub issue");
    exposure.index_tool("bash", "shell");
    let bridge = Arc::new(GuardedBridge::with_exposure(
        Arc::new(StubBridge {
            calls: Mutex::new(Vec::new()),
            responses,
        }),
        Some(allow),
        32,
        Some(Arc::clone(&exposure)),
    ));
    let script = r#"
hits = search_tools("github")
out = call_tool(hits[0]["name"], {"title": "x"})
result = {"found": hits[0]["name"], "content": out["content"]}
"#;
    let output = tokio::task::block_in_place(|| {
        evaluate_code_mode_with_exposure(
            script,
            Arc::clone(&bridge),
            EngineLimits::default(),
            Some(exposure),
        )
    })
    .expect("evaluate");
    assert_eq!(
        output.return_value["found"],
        json!("mcp__github__create_issue")
    );
    assert_eq!(output.return_value["content"], json!("created"));
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_tool_unreachable_from_script() {
    use super::engine::evaluate_code_mode_with_exposure;
    use super::exposure::{ExposureController, ExposurePolicy, ToolExposure};

    let policy = ExposurePolicy::new().override_exact("secret", ToolExposure::Hidden);
    let exposure = Arc::new(ExposureController::new(policy));
    exposure.index_tool("secret", "nope");
    let bridge = Arc::new(GuardedBridge::with_exposure(
        Arc::new(StubBridge {
            calls: Mutex::new(Vec::new()),
            responses: BTreeMap::new(),
        }),
        None,
        32,
        Some(Arc::clone(&exposure)),
    ));
    let err = tokio::task::block_in_place(|| {
        evaluate_code_mode_with_exposure(
            r#"result = call_tool("secret", {})"#,
            bridge,
            EngineLimits::default(),
            Some(exposure),
        )
    })
    .expect_err("hidden");
    assert!(err.to_string().contains("hidden"), "unexpected: {err}");
}
