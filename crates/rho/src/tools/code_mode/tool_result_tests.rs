use pretty_assertions::assert_eq;
use serde_json::{json, Value as JsonValue};
use starlark::environment::{Globals, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};

use super::alloc_tool_result;

/// Runs `source` with `r` and `same` bound to equal envelopes of `envelope`,
/// returning `result` as JSON or the failure text.
fn run(envelope: &JsonValue, source: &str) -> Result<JsonValue, String> {
    let ast = AstModule::parse("test.star", source.to_owned(), &Dialect::Standard)
        .map_err(|error| error.to_string())?;
    Module::with_temp_heap(|module| {
        module.set("r", alloc_tool_result(module.heap(), envelope));
        module.set("same", alloc_tool_result(module.heap(), envelope));
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &Globals::standard())?;
        let result = module.get("result").expect("result");
        result.to_json_value().map_err(starlark::Error::new_other)
    })
    .map_err(|error: starlark::Error| error.to_string())
}

// Covers: the envelope reads both as `r.content` (what JavaScript-trained
// models write) and `r["content"]` (the original dict API), prints and
// serializes like the dict it replaced, and keeps `==` symmetric with dicts.
// Owner: codemode result envelope.
#[test]
fn envelope_reads_as_attributes_and_keys() {
    let envelope = json!({"is_error": false, "content": "text", "data": {"n": 1}});
    for (source, expected) in [
        (
            "result = [r.is_error, r.content, r.data]",
            json!([false, "text", {"n": 1}]),
        ),
        (
            "result = [r[\"is_error\"], r[\"content\"], r[\"data\"]]",
            json!([false, "text", {"n": 1}]),
        ),
        ("result = [\"data\" in r, \"other\" in r]", json!([true, false])),
        ("result = r", envelope.clone()),
        (
            "result = repr(r)",
            json!("{\"is_error\": False, \"content\": \"text\", \"data\": {\"n\": 1}}"),
        ),
        (
            "d = {\"is_error\": False, \"content\": \"text\", \"data\": {\"n\": 1}}\nresult = [r == same, r == d, d == r]",
            json!([true, false, false]),
        ),
    ] {
        assert_eq!(run(&envelope, source), Ok(expected), "{source}");
    }
    for source in ["result = r.missing", "result = r[\"missing\"]"] {
        assert!(run(&envelope, source).is_err(), "{source}");
    }
}
