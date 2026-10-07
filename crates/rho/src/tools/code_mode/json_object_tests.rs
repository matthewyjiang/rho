use pretty_assertions::assert_eq;
use serde_json::{json, Value as JsonValue};
use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

use super::alloc_json;

/// Runs `source` with `obj` bound to `json`, returning `result` as JSON.
fn run(json: &JsonValue, source: &str) -> Result<JsonValue, String> {
    let ast = AstModule::parse("test.star", source.to_owned(), &Dialect::Standard)
        .map_err(|error| error.to_string())?;
    Module::with_temp_heap(|module| {
        module.set("obj", alloc_json(module.heap(), json));
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &Globals::standard())?;
        let result = module.get("result").expect("result");
        result.to_json_value().map_err(starlark::Error::new_other)
    })
    .map_err(|error: starlark::Error| error.to_string())
}

// Covers: tool results read the way JavaScript-trained models write them
// (`r.content`, `r.data.next_cursor`) while keeping full dict behavior, so
// subscripting, dict methods, iteration, equality, mutation, and JSON output
// all still work at every depth.
// Owner: codemode JSON values handed to scripts.
#[test]
fn json_objects_are_dicts_with_attribute_reads() {
    let json = json!({
        "content": "text",
        "items": [1],
        "data": {"next_cursor": 3, "nested": {"k": "v"}},
    });
    for (source, expected) in [
        ("result = obj.content", json!("text")),
        ("result = obj[\"content\"]", json!("text")),
        ("result = obj.data.next_cursor", json!(3)),
        ("result = obj[\"data\"].nested.k", json!("v")),
        // Keys win over dict methods; methods stay reachable otherwise.
        ("result = obj.items", json!([1])),
        ("result = obj.data.get(\"missing\", 0)", json!(0)),
        (
            "result = sorted(obj.data.keys())",
            json!(["nested", "next_cursor"]),
        ),
        (
            "result = [k for k in obj.data]",
            json!(["next_cursor", "nested"]),
        ),
        (
            "result = (len(obj), \"data\" in obj, bool(obj))",
            json!([3, true, true]),
        ),
        (
            "result = obj.data == {\"next_cursor\": 3, \"nested\": {\"k\": \"v\"}}",
            json!(true),
        ),
        ("result = obj.data.nested == obj.data.nested", json!(true)),
        ("obj.data[\"extra\"] = 1\nresult = obj.data.extra", json!(1)),
        ("result = obj", json.clone()),
    ] {
        assert_eq!(run(&json, source), Ok(expected), "{source}");
    }
    assert!(run(&json, "result = obj.missing").is_err());
}
