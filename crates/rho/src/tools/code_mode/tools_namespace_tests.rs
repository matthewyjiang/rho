use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolSpec,
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput},
    ToolHost,
};
use serde_json::json;

use super::super::{
    live_wiring_tests::{script, surface},
    tool::CodeModeTool,
};

/// Echo arguments while recording the canonical names actually dispatched.
struct EchoTool {
    name: &'static str,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Tool for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "fixture echo".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        let arguments = invocation.arguments().clone();
        Box::pin(async move {
            self.calls.lock().unwrap().push(self.name.to_owned());
            Ok(ToolOutput::text("echo").with_structured_content(arguments))
        })
    }
}

fn host(names: &[&'static str]) -> (ToolHost, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tools = names
        .iter()
        .map(|&name| {
            Arc::new(EchoTool {
                name,
                calls: calls.clone(),
            }) as Arc<dyn Tool>
        })
        .collect();
    let host = ToolHost::builder()
        .tool(CodeModeTool::new(surface(tools)))
        .build()
        .unwrap();
    (host, calls)
}

// Covers: the Codex-shaped script a GPT model writes first runs as written:
// `tools.<name>(...)` with a dict and/or keywords, a normalized name, and
// attribute reads on direct and batched results.
// Owner: codemode namespace / ToolHost integration.
#[tokio::test]
async fn namespace_calls_adapt_arguments() {
    let (host, _) = host(&["get-docs"]);
    for (source, expected) in [
        (
            "r = tools.get_docs({\"a\": 1, \"b\": 1}, b=2)\nresult = [r.content, r.data]",
            Ok(json!(["echo", {"a": 1, "b": 2}])),
        ),
        (
            "rs = call_tools([(\"get-docs\", {\"n\": 1}), (\"get-docs\", {\"n\": 2})])\nresult = [r.data[\"n\"] for r in rs]",
            Ok(json!([1, 2])),
        ),
        ("result = tools.missing()", Err(())),
        ("result = tools.get_docs([1])", Err(())),
    ] {
        assert_eq!(script(&host, source).await.map_err(|_| ()), expected, "{source}");
    }
}

// Covers: alias collisions never start a tool; exact dispatch remains available
// and an exact identifier wins regardless of catalog order.
// Owner: codemode namespace resolution.
#[tokio::test]
async fn ambiguous_aliases_do_not_dispatch() {
    for names in [vec!["get-docs", "get.docs"], vec!["get.docs", "get-docs"]] {
        let (host, calls) = host(&names);
        assert!(script(&host, "result = tools.get_docs()").await.is_err());
        assert_eq!(*calls.lock().unwrap(), Vec::<String>::new());
        assert_eq!(
            script(&host, "result = [call_tool(\"get-docs\")[\"content\"], call_tool(\"get.docs\")[\"content\"]]").await,
            Ok(json!(["echo", "echo"]))
        );
        assert_eq!(*calls.lock().unwrap(), vec!["get-docs", "get.docs"]);
    }
    for names in [
        vec!["get-docs", "get.docs", "get_docs"],
        vec!["get_docs", "get.docs", "get-docs"],
    ] {
        let (host, calls) = host(&names);
        assert_eq!(
            script(&host, "result = tools.get_docs()[\"content\"]").await,
            Ok(json!("echo"))
        );
        assert_eq!(*calls.lock().unwrap(), vec!["get_docs"]);
    }
}

// Covers: tool data stays native dicts behind the result envelope, including
// self-update, keyword expansion, and symmetric/nested content equality.
// Owner: codemode JSON allocation at the script API boundary.
#[tokio::test]
async fn tool_and_discovery_objects_keep_native_dictionary_contracts() {
    let (host, _) = host(&["get-docs"]);
    let data = json!({"n": 1, "nested": {"k": "v"}});
    for call in [
        "call_tool(\"get-docs\", payload)",
        "call_tools([(\"get-docs\", payload)])[0]",
        "tools.get_docs(payload)",
    ] {
        for (body, expected) in [
            ("result = dict(r[\"data\"])", data.clone()),
            ("d = {}\nd.update(r[\"data\"])\nresult = d", data.clone()),
            ("r[\"data\"].update(r[\"data\"])\nresult = r[\"data\"]", data.clone()),
            ("def f(n, nested):\n    return {\"n\": n, \"nested\": nested}\nresult = f(**r[\"data\"])", data.clone()),
            ("result = {} | r[\"data\"]", data.clone()),
            ("result = [r[\"data\"] == payload, payload == r[\"data\"], [payload] == [r[\"data\"]], [r[\"data\"]] == [payload]]", json!([true, true, true, true])),
        ] {
            let source = format!("payload = {{\"n\": 1, \"nested\": {{\"k\": \"v\"}}}}\nr = {call}\n{body}");
            assert_eq!(script(&host, &source).await, Ok(expected), "{source}");
        }
    }
    for source in [
        "result = dict(list_tools()[0]) == list_tools()[0]",
        "result = dict(search_tools(\"get-docs\")[0]) == search_tools(\"get-docs\")[0]",
        "result = dict(describe_tool(\"get-docs\")) == describe_tool(\"get-docs\")",
    ] {
        assert_eq!(script(&host, source).await, Ok(json!(true)), "{source}");
    }
}
