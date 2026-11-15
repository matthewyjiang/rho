//! Runs the vendored agent-side suites for A2UI Express and catalog
//! transformers against the crate: one test per suite.

use std::fs;
use std::path::Path;

use rho_a2ui::{
    AllowedMessage, Catalog, ErrorKind, Express, ExpressError, Framing, PromptOptions,
    DEFAULT_SURFACE_ID,
};
use serde_json::{Map, Value};

/// Cases the harness does not run: the suite, the case and the reason. An
/// entry that names no case of its suite fails that suite.
const SKIPPED: &[(&str, &str, &str)] = &[];

/// Cases each suite leaves out because the catalog they load targets a
/// protocol version before 1.0. A vendor refresh that changes them fails
/// here, to be checked.
const FILTERED_COMPILER: usize = 3;
const FILTERED_DECOMPILER: usize = 1;
const FILTERED_PROMPT: usize = 2;
const FILTERED_TRANSFORMER: usize = 0;

fn conformance_path(path: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/a2ui/conformance")
        .join(path)
}

fn load_suite(file: &str) -> Vec<Value> {
    let path = conformance_path(file);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let suite: Value = serde_yaml_ng::from_str(&text)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
    match suite {
        Value::Array(cases) if !cases.is_empty() => cases,
        other => panic!("{} is not a list of cases: {other}", path.display()),
    }
}

fn read_json(path: &str) -> Value {
    let full = conformance_path(path);
    let text = fs::read_to_string(&full).unwrap_or_else(|e| {
        panic!(
            "reading {}: {e} (is it in vendor/a2ui/FILES?)",
            full.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", full.display()))
}

fn case_name(case: &Value) -> &str {
    case["name"].as_str().expect("every case has a name")
}

/// Fails on a key of `object` that `known` does not list, so that a suite
/// refresh adding an expectation is not silently ignored.
fn check_keys(object: &Value, known: &[&str], what: &str) -> Result<(), String> {
    let map = object
        .as_object()
        .ok_or(format!("{what} is not an object"))?;
    match map.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("unknown {what} key {key:?}")),
        None => Ok(()),
    }
}

/// The catalog documents a case registers, each with the transformers
/// applied to it, as `args.catalog` or `args.catalogs` give them.
fn registrations(args: &Value) -> Vec<&Value> {
    match (args.get("catalog"), args.get("catalogs")) {
        (Some(one), None) => vec![one],
        (None, Some(Value::Array(many))) => many.iter().collect(),
        (None, None) => Vec::new(),
        other => panic!("unexpected catalog arguments {other:?}"),
    }
}

/// A fixture document, by path or inline. One that omits `catalogId` takes
/// its path, and one that omits `protocolVersion` targets 1.0, as the
/// harness guide says.
fn fixture_document(spec: &Value) -> Value {
    let (mut document, path) = match spec {
        Value::String(path) => (read_json(path), Some(path.clone())),
        Value::Object(_) => (spec.clone(), None),
        other => panic!("unexpected catalog {other}"),
    };
    let map = document.as_object_mut().expect("a catalog is an object");
    if let Some(path) = path {
        map.entry("catalogId").or_insert(Value::String(path));
    }
    map.entry("protocolVersion")
        .or_insert(Value::String("1.0".to_owned()));
    document
}

/// The document of a registration: a bare catalog, or `{catalog,
/// transformers}`.
fn registration_document(registration: &Value) -> &Value {
    match registration.get("catalog") {
        Some(catalog) if registration.get("components").is_none() => catalog,
        _ => registration,
    }
}

/// Loads a registration and applies its transformers in order.
fn load_catalog(registration: &Value) -> Result<Catalog, String> {
    let document = fixture_document(registration_document(registration));
    let mut catalog = Catalog::from_json(document).map_err(|e| format!("loading catalog: {e}"))?;
    let transformers = registration.get("transformers").and_then(Value::as_array);
    for transformer in transformers.into_iter().flatten() {
        check_keys(
            transformer,
            &["component_pruning", "function_pruning"],
            "transformer",
        )?;
        let allowlist = |key: &str| -> Option<Vec<String>> {
            let names = transformer.get(key)?;
            Some(
                names
                    .as_array()
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        };
        if let Some(names) = allowlist("component_pruning") {
            catalog = catalog
                .keep_components(&names)
                .map_err(|e| format!("pruning components: {e}"))?;
        }
        if let Some(names) = allowlist("function_pruning") {
            catalog = catalog
                .keep_functions(&names)
                .map_err(|e| format!("pruning functions: {e}"))?;
        }
    }
    Ok(catalog)
}

fn load_catalogs(args: &Value) -> Result<Vec<Catalog>, String> {
    registrations(args).into_iter().map(load_catalog).collect()
}

/// The protocol version a case runs under: that of the first catalog it
/// loads, 1.0 when it loads none.
fn case_version(case: &Value) -> String {
    let first = registrations(&case["args"]).into_iter().next();
    first.map_or_else(
        || "1.0".to_owned(),
        |registration| {
            fixture_document(registration_document(registration))["protocolVersion"]
                .as_str()
                .expect("protocolVersion is a string")
                .to_owned()
        },
    )
}

/// Runs the cases of `file` that target v1.0, collecting failures, and
/// asserts how many it left out as earlier versions.
fn run_suite(file: &str, filtered: usize, run: fn(&Value) -> Result<(), String>) {
    let mut failures = Vec::new();
    let mut left_out = 0;
    let mut ran = 0;
    let cases = load_suite(file);
    let skipped: Vec<&str> = SKIPPED
        .iter()
        .filter(|(suite, _, _)| *suite == file)
        .map(|(_, name, _)| *name)
        .collect();
    for name in &skipped {
        assert!(
            cases.iter().any(|case| case_name(case) == *name),
            "{file}: SKIPPED names {name}, which the suite lacks"
        );
    }
    for case in cases {
        let name = case_name(&case);
        match case_version(&case).as_str() {
            "1.0" => {}
            "0.9" | "v0.8" | "v0.9" | "v0.9.1" => {
                left_out += 1;
                continue;
            }
            other => panic!("{name}: unknown protocol version {other:?}"),
        }
        if skipped.contains(&name) {
            continue;
        }
        ran += 1;
        if let Err(reason) = run(&case) {
            failures.push(format!("{name}: {reason}"));
        }
    }
    assert!(ran > 0, "{file}: no case ran");
    assert!(
        failures.is_empty(),
        "{file}: {} failing:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(left_out, filtered, "{file}: cases left out as pre-1.0");
}

/// Whether two JSON values are equal, numbers compared by value: the suites
/// write `100` where an implementation may give `100.0`.
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| same_json(value, other)))
        }
        _ => a == b,
    }
}

/// The error category a case expects, as an [`ErrorKind`]. Its `message`,
/// free text, is not compared: the kind is.
fn expected_kind(case: &Value) -> Result<Option<ErrorKind>, String> {
    let Some(want) = case.get("expect_error") else {
        return Ok(None);
    };
    check_keys(want, &["category", "message"], "expect_error")?;
    match want["category"].as_str() {
        Some("ParseError") => Ok(Some(ErrorKind::Parse)),
        Some("ValidationError") => Ok(Some(ErrorKind::Validation)),
        Some("CatalogError") => Ok(Some(ErrorKind::Catalog)),
        other => Err(format!("unknown error category {other:?}")),
    }
}

/// Checks an outcome against the case's `expect_error`, returning the value
/// to check further when the case expects success.
fn check_outcome<T>(case: &Value, outcome: Result<T, ExpressError>) -> Result<Option<T>, String> {
    match (outcome, expected_kind(case)?) {
        (Ok(value), None) => Ok(Some(value)),
        (Ok(_), Some(kind)) => Err(format!("succeeded, want a {kind:?} error")),
        (Err(e), None) => Err(format!("failed: {e}")),
        (Err(e), Some(kind)) if e.kind() == kind => Ok(None),
        (Err(e), Some(kind)) => Err(format!("failed with {:?} ({e}), want {kind:?}", e.kind())),
    }
}

fn express(args: &Value) -> Result<Express, String> {
    Express::new(load_catalogs(args)?).map_err(|e| format!("building Express: {e}"))
}

#[test]
fn compiler_suite() {
    run_suite(
        "agent/express/compiler.yaml",
        FILTERED_COMPILER,
        run_compile,
    );
}

fn run_compile(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "args",
            "input",
            "expect",
            "expect_error",
            "expect_present",
        ],
        "case",
    )?;
    if case["action"] != "compile" {
        return Err(format!("unknown action {}", case["action"]));
    }
    let format = express(&case["args"])?;
    let input = case["input"].as_str().ok_or("input is not a string")?;
    let Some(mut got) =
        check_outcome(case, format.compile(input, DEFAULT_SURFACE_ID))?.map(Value::Array)
    else {
        return Ok(());
    };
    for pointer in case
        .get("expect_present")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let pointer = pointer
            .as_str()
            .ok_or("expect_present holds a non-string")?;
        let present = got.pointer(pointer).is_some_and(|v| {
            !v.is_null() && v.as_str() != Some("") && v != &Value::Object(Map::new())
        });
        if !present {
            return Err(format!("{pointer} is missing or empty in {got}"));
        }
        let (parent, key) = pointer.rsplit_once('/').expect("a pointer has a parent");
        if let Some(Value::Object(map)) = got.pointer_mut(parent) {
            map.remove(key);
        }
    }
    if same_json(&got, &case["expect"]) {
        Ok(())
    } else {
        Err(format!("compiled to\n {got}\nwant\n {}", case["expect"]))
    }
}

#[test]
fn decompiler_suite() {
    run_suite(
        "agent/express/decompiler.yaml",
        FILTERED_DECOMPILER,
        run_decompile,
    );
}

fn run_decompile(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "args",
            "messages",
            "expect_output",
            "expect_contains",
            "expect_absent",
            "expect_round_trip",
            "expect_recompiled",
            "expect_error",
        ],
        "case",
    )?;
    if case["action"] != "decompile" {
        return Err(format!("unknown action {}", case["action"]));
    }
    let format = express(&case["args"])?;
    let messages = case["messages"]
        .as_array()
        .ok_or("messages is not a list")?;
    let Some(output) = check_outcome(case, format.decompile(messages))? else {
        return Ok(());
    };
    check_text(case, &output)?;
    let recompiled = || {
        express(&case["args"])?
            .compile(&output, DEFAULT_SURFACE_ID)
            .map(Value::Array)
            .map_err(|e| format!("recompiling {output:?}: {e}"))
    };
    if case.get("expect_round_trip") == Some(&Value::Bool(true)) {
        let got = recompiled()?;
        if !same_json(&got, &case["messages"]) {
            return Err(format!(
                "{output:?} recompiled to\n {got}\nwant\n {}",
                case["messages"]
            ));
        }
    }
    if let Some(want) = case.get("expect_recompiled") {
        let got = recompiled()?;
        if !same_json(&got, want) {
            return Err(format!("{output:?} recompiled to\n {got}\nwant\n {want}"));
        }
    }
    Ok(())
}

/// Checks `expect_output`, `expect_contains` and `expect_absent` against a
/// text output.
fn check_text(case: &Value, output: &str) -> Result<(), String> {
    if let Some(want) = case.get("expect_output") {
        if want.as_str() != Some(output) {
            return Err(format!("wrote {output:?}, want {want}"));
        }
    }
    let fragments = |key: &str| -> Vec<&str> {
        case.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    };
    if let Some(missing) = fragments("expect_contains")
        .into_iter()
        .find(|f| !output.contains(f))
    {
        return Err(format!("missing {missing:?} in\n{output}"));
    }
    if let Some(present) = fragments("expect_absent")
        .into_iter()
        .find(|f| output.contains(f))
    {
        return Err(format!("has {present:?} in\n{output}"));
    }
    Ok(())
}

#[test]
fn prompt_generator_suite() {
    run_suite(
        "agent/express/prompt_generator.yaml",
        FILTERED_PROMPT,
        run_prompt,
    );
}

fn run_prompt(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "args",
            "expect_contains",
            "expect_absent",
            "expect_deterministic",
            "expect_callable_functions",
            "expect_error",
        ],
        "case",
    )?;
    if case["action"] != "generate_prompt_snippet" {
        return Err(format!("unknown action {}", case["action"]));
    }
    let args = &case["args"];
    check_keys(
        args,
        &["format", "catalogs", "examples", "allowed_messages"],
        "args",
    )?;
    let examples = args
        .get("examples")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|path| {
            let turn = read_json(path.as_str().expect("an example is a path"));
            turn.as_array().cloned().unwrap_or_else(|| vec![turn])
        })
        .collect();
    let allowed = args
        .get("allowed_messages")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .map(|name| {
                    name.as_str()
                        .and_then(AllowedMessage::from_wire_name)
                        .ok_or(format!("unknown message {name}"))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?;
    let options = PromptOptions {
        framing: Framing::Tagged,
        allowed,
        examples,
    };
    let catalogs = load_catalogs(args)?;
    let generate = || Express::new(catalogs.clone())?.prompt(&options);
    let Some(prompt) = check_outcome(case, generate())? else {
        return Ok(());
    };
    check_text(case, &prompt)?;
    if case.get("expect_deterministic") == Some(&Value::Bool(true))
        && generate().map_err(|e| e.to_string())? != prompt
    {
        return Err("a second prompt differs from the first".to_owned());
    }
    if let Some(want) = case.get("expect_callable_functions") {
        let format = express(args)?;
        let got: Vec<&str> = format.agent_callable_functions();
        if Value::from(got.clone()) != *want {
            return Err(format!("callable functions {got:?}, want {want}"));
        }
        if let Some(name) = got.iter().find(|name| !prompt.contains(*name)) {
            return Err(format!("the prompt does not name callable function {name}"));
        }
    }
    Ok(())
}

#[test]
fn catalog_transformer_suite() {
    run_suite(
        "agent/catalog_transformer.yaml",
        FILTERED_TRANSFORMER,
        run_transform,
    );
}

fn run_transform(case: &Value) -> Result<(), String> {
    check_keys(
        case,
        &[
            "name",
            "description",
            "features",
            "action",
            "args",
            "expect",
        ],
        "case",
    )?;
    if case["action"] != "transform_catalog" {
        return Err(format!("unknown action {}", case["action"]));
    }
    let catalog = load_catalog(&case["args"])?;
    let expect = &case["expect"];
    check_keys(
        expect,
        &["catalog_id", "protocol_version", "components", "functions"],
        "expect",
    )?;
    if let Some(id) = expect.get("catalog_id") {
        if id.as_str() != Some(catalog.id()) {
            return Err(format!("catalog id {:?}, want {id}", catalog.id()));
        }
    }
    if let Some(version) = expect.get("protocol_version") {
        if version.as_str() != Some(catalog.protocol_version()) {
            return Err(format!(
                "protocol version {:?}, want {version}",
                catalog.protocol_version()
            ));
        }
    }
    let names = |want: &Value| -> Vec<String> {
        let mut names: Vec<String> = want
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        names
    };
    if let Some(want) = expect.get("components") {
        let got: Vec<String> = catalog.components().keys().cloned().collect();
        if got != names(want) {
            return Err(format!("components {got:?}, want {want}"));
        }
    }
    if let Some(want) = expect.get("functions") {
        let got: Vec<String> = catalog.functions().keys().cloned().collect();
        if got != names(want) {
            return Err(format!("functions {got:?}, want {want}"));
        }
    }
    Ok(())
}
