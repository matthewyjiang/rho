//! Runs the vendored upstream conformance suites against the crate.

#[path = "conformance/processing.rs"]
mod processing;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rho_a2ui::{
    parse_template, resolve_path, DataContext, DataModel, Expr, ExpressionError, FunctionError,
    Functions, Pointer, WatchId,
};
use rho_a2ui::{validate_common_type, Catalog, CatalogError, CatalogValidator, Derivation, Entry};
use serde_json::{json, Map, Value};

/// Cases the harness cannot express in Rust, with the reason. A case here is
/// not run.
const SKIPPED: &[(&str, &str)] = &[
    (
        "test_v10_get_renderer_data_model_excludes_surface_without_default_from_v09",
        "asks for a v0.9 data model; the crate holds only v1.0 surfaces",
    ),
    (
        "test_v10_get_renderer_data_model_filters_by_surface_protocol_version",
        "creates a v0.9 surface; the crate processes only v1.0 messages",
    ),
];

fn load_suite(file: &str) -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/a2ui/conformance/core")
        .join(file);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let suite: Value = serde_yaml_ng::from_str(&text)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
    match suite {
        Value::Array(cases) => cases,
        other => panic!("{} is not a list of cases: {other}", path.display()),
    }
}

fn case_name(case: &Value) -> &str {
    case["name"].as_str().expect("every case has a name")
}

fn assert_no_failures(suite: &str, failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{suite}: {} failing:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every skipped case is a vendored one, so that an entry a vendor refresh
/// leaves stale is noticed.
#[test]
fn skipped_cases_exist() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/a2ui/conformance/core");
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .flat_map(|file| load_suite(&file))
        .map(|case| case_name(&case).to_owned())
        .collect();
    for (name, _) in SKIPPED {
        assert!(
            names.iter().any(|known| known == name),
            "SKIPPED names no case {name:?}"
        );
    }
}

fn is_skipped(name: &str) -> bool {
    SKIPPED.iter().any(|(skipped, _)| *skipped == name)
}

/// A case's error expectation, under either spelling the suites use.
fn expected_error(case: &Value) -> Option<&Value> {
    case.get("expect_error").or_else(|| case.get("expectError"))
}

#[test]
fn expressions_suite() {
    let mut failures = Vec::new();
    for case in load_suite("expressions.yaml") {
        let name = case_name(&case);
        if is_skipped(name) {
            continue;
        }
        let verdict = match case["action"].as_str() {
            Some("parse_expression_template") => run_template_case(&case),
            Some("validate") => processing::run_case(&case),
            _ => Err(format!("unknown action {}", case["action"])),
        };
        if let Err(reason) = verdict {
            failures.push(format!("{name}: {reason}"));
        }
    }
    assert_no_failures("expressions", &failures);
}

fn run_template_case(case: &Value) -> Result<(), String> {
    let input = case["input"].as_str().ok_or("input is not a string")?;
    let outcome =
        parse_template(input).map(|parts| Value::Array(parts.iter().map(part_json).collect()));
    match (outcome, expected_error(case)) {
        (Ok(got), None) if got == case["expect"] => Ok(()),
        (Ok(got), None) => Err(format!("got {got}, want {}", case["expect"])),
        (Ok(got), Some(_)) => Err(format!("parsed to {got}, want an error")),
        (Err(e), None) => Err(format!("failed: {e}")),
        (Err(e), Some(want)) => check_parse_error(&e, want),
    }
}

/// A template part in the suite's notation.
fn part_json(part: &Expr) -> Value {
    match part {
        Expr::Literal(value) => value.clone(),
        Expr::Path(path) => json!({ "path": path }),
        Expr::Call(call) => json!({
            "call": call.name,
            "args": call.args.iter().map(|(k, v)| (k.clone(), part_json(v))).collect::<Map<_, _>>(),
            "returnType": "any",
        }),
        Expr::List(_) | Expr::Object(_) => panic!("templates have no containers: {part:?}"),
    }
}

/// Checks a parse error against the suite's `ParseError` category and the
/// message phrase that names its cause.
fn check_parse_error(error: &ExpressionError, want: &Value) -> Result<(), String> {
    if want["category"] != "ParseError" {
        return Err(format!("unknown error category {}", want["category"]));
    }
    let phrase = parse_error_phrase(error);
    if want["message"].as_str() == phrase {
        Ok(())
    } else {
        Err(format!(
            "failed with {error:?} ({phrase:?}), want {}",
            want["message"]
        ))
    }
}

/// The phrase the suite's `expect_error.message` uses for each cause it
/// tests, so a case is matched by cause rather than by wording.
fn parse_error_phrase(error: &ExpressionError) -> Option<&'static str> {
    use ExpressionError::*;
    match error {
        UnexpectedCharacters { .. } => Some("Unexpected characters"),
        MissingArgumentColon { .. } => Some("after argument name"),
        UnclosedCall { .. } => Some("after function arguments"),
        UnclosedInterpolation => Some("Unclosed interpolation"),
        InvalidNumber { .. } => Some("Invalid number literal"),
        NumberOutOfRange { .. } => Some("out of range"),
        TooDeep => Some("Max recursion depth"),
        UnclosedString
        | InvalidPathEscape { .. }
        | TildeInFunctionName { .. }
        | InvalidSystemName
        | SystemNameWithoutCall { .. }
        | TemplateTooLong { .. }
        | TooManyParts
        | UnknownDirective { .. }
        | MalformedBinding
        | MalformedCall
        | InvalidPath(_)
        | UnknownFunction { .. }
        | IndexOutsideCollection
        | EvaluationTooDeep
        | UnknownSystemFunction { .. }
        | UnresolvedCatalog(_)
        | AgentOnly { .. }
        | UserActivationRequired { .. }
        | Function { .. } => None,
    }
}

/// `evaluate_function` cases on a surface for a version before 1.0, which
/// the functions suite leaves out.
const FILTERED_FUNCTIONS: usize = 1;

#[test]
fn functions_suite() {
    let functions = Functions::basic();
    let mut failures = Vec::new();
    let mut left_out = 0;
    for case in load_suite("functions.yaml") {
        let name = case_name(&case);
        if is_skipped(name) {
            continue;
        }
        let on_surface = ["surface", "catalogId", "catalogPaths", "catalog"]
            .iter()
            .any(|key| case.get(key).is_some());
        let verdict = match case["action"].as_str() {
            Some("validate") => processing::run_case(&case),
            Some("evaluate_function") if on_surface => {
                if processing::case_version(&case, "functions.yaml") != "1.0" {
                    left_out += 1;
                    continue;
                }
                processing::run_function_on_surface(&case)
            }
            Some("evaluate_function") => run_function_case(&case, &functions),
            _ => Err(format!("unknown action {}", case["action"])),
        };
        if let Err(reason) = verdict {
            failures.push(format!("{name}: {reason}"));
        }
    }
    assert_no_failures("functions", &failures);
    assert_eq!(
        left_out, FILTERED_FUNCTIONS,
        "functions: cases left out as pre-1.0"
    );
}

/// Calls the case's function with its arguments, at the root of its data.
fn run_function_case(case: &Value, functions: &Functions) -> Result<(), String> {
    let model = DataModel::new(case.get("dataModel").cloned().unwrap_or_else(|| json!({})));
    let call = json!({"@call": case["function"], "args": case.get("args").cloned().unwrap_or_else(|| json!({}))});
    let outcome =
        Expr::from_json(&call).and_then(|call| DataContext::new(&model, functions).evaluate(&call));
    match (outcome, expected_error(case)) {
        (Ok(got), None) => check_function_result(case, &got),
        (Ok(got), Some(_)) => Err(format!("returned {got}, want an error")),
        (Err(e), None) => Err(format!("failed: {e}")),
        (Err(e), Some(want)) => check_function_error(&e, want),
    }
}

/// Compares a result with `expect`.
///
/// Validators return a `ValidationResult`. A v1.0 case states the members it
/// checks, such as `{valid: false}`, and only those are compared. A case for
/// an earlier version, written when validators returned a boolean, states
/// that boolean, which is the result's `valid`.
fn check_function_result(case: &Value, got: &Value) -> Result<(), String> {
    let want = &case["expect"];
    let version = case
        .get("protocolVersion")
        .or_else(|| case.pointer("/catalog/protocolVersion"));
    let validity = got.get("valid").filter(|valid| valid.is_boolean());
    let matches = match (version.and_then(Value::as_str), want, validity) {
        (Some("1.0"), Value::Object(members), Some(_)) if members.contains_key("valid") => members
            .iter()
            .all(|(key, value)| got.get(key) == Some(value)),
        (Some("1.0"), _, _) => got == want,
        (_, Value::Bool(_), Some(valid)) => valid == want,
        _ => got == want,
    };
    if matches {
        Ok(())
    } else {
        Err(format!("got {got}, want {want}"))
    }
}

/// Checks an error against the suite's `ExpressionError` category and, when
/// the case names one, the regex dialect's diagnostic token.
fn check_function_error(error: &ExpressionError, want: &Value) -> Result<(), String> {
    if want["category"] != "ExpressionError" {
        return Err(format!(
            "failed with {error:?}, want a {}",
            want["category"]
        ));
    }
    let Some(message) = want.get("message") else {
        return Ok(());
    };
    let code = match error {
        ExpressionError::Function {
            source: FunctionError::Regex(regex),
            ..
        } => regex.code(),
        _ => None,
    };
    if message.as_str() == code {
        Ok(())
    } else {
        Err(format!("failed with {error:?}, want {message}"))
    }
}

#[test]
fn data_context_suite() {
    let mut failures = Vec::new();
    for case in load_suite("data_context.yaml") {
        let name = case_name(&case);
        if is_skipped(name) {
            continue;
        }
        assert_eq!(case["action"], "resolve_path", "{name}: unknown action");
        let args = &case["args"];
        let got = resolve_path(
            args["path"].as_str().expect("path argument"),
            args["contextPath"].as_str(),
        );
        if case["expect"] != got.as_str() {
            failures.push(format!("{name}: got {got:?}, want {}", case["expect"]));
        }
    }
    assert_no_failures("data_context", &failures);
}

#[test]
fn data_model_suite() {
    let mut failures = Vec::new();
    for case in load_suite("data_model.yaml") {
        let name = case_name(&case);
        if is_skipped(name) {
            continue;
        }
        if let Err(reason) = run_data_model_case(&case) {
            failures.push(format!("{name}: {reason}"));
        }
    }
    assert_no_failures("data_model", &failures);
}

/// Runs one case, stopping at its first failing step.
fn run_data_model_case(case: &Value) -> Result<(), String> {
    let initial = case
        .get("initial")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut model = DataModel::new(initial);

    let mut watched: HashMap<WatchId, Pointer> = HashMap::new();
    for path in case["watch"].as_array().into_iter().flatten() {
        let pointer = parse(path.as_str().ok_or("watch path is not a string")?)?;
        watched.insert(model.watch(&pointer), pointer);
    }

    for (n, step) in case["steps"]
        .as_array()
        .ok_or("no steps")?
        .iter()
        .enumerate()
    {
        run_step(&mut model, &watched, step).map_err(|reason| format!("step {n}: {reason}"))?;
    }
    Ok(())
}

fn parse(path: &str) -> Result<Pointer, String> {
    Pointer::parse(path).map_err(|e| e.to_string())
}

fn run_step(
    model: &mut DataModel,
    watched: &HashMap<WatchId, Pointer>,
    step: &Value,
) -> Result<(), String> {
    let op = step["op"].as_str().ok_or("step has no op")?;
    let path = step["path"].as_str().unwrap_or("/");
    let expects_error = step.get("expect_error").is_some();

    let fired = match op {
        "get" => {
            return check_get(model, step, path);
        }
        "set" | "delete" => {
            let value = if op == "set" {
                step["value"].clone()
            } else {
                Value::Null
            };
            let before = model.clone();
            let outcome = parse(path)
                .and_then(|pointer| model.set(&pointer, value).map_err(|e| e.to_string()));
            match (outcome, expects_error) {
                (Ok(fired), false) => fired,
                (Err(_), true) => {
                    return if before.get(&Pointer::root()) == model.get(&Pointer::root()) {
                        Ok(())
                    } else {
                        Err("model changed by a failed write".to_owned())
                    };
                }
                (Ok(_), true) => return Err(format!("{op} {path} succeeded, want an error")),
                (Err(e), false) => return Err(format!("{op} {path} failed: {e}")),
            }
        }
        "dispose" => {
            model.dispose();
            Vec::new()
        }
        // The suite hands `set` an object and then mutates it. Rust's `set`
        // takes the value by move, so no caller can reach committed state.
        "mutate_input" => Vec::new(),
        other => return Err(format!("unknown op {other:?}")),
    };

    if let Some(expected) = step.get("expect_notified") {
        check_notified(watched, &fired, expected)?;
    }
    if let Some(expected) = step.get("expect_values") {
        for (path, want) in expected.as_object().ok_or("expect_values is not a map")? {
            let got = model.get(&parse(path)?);
            if got != Some(want) {
                return Err(format!("{path} holds {got:?}, want {want}"));
            }
        }
    }
    Ok(())
}

fn check_get(model: &DataModel, step: &Value, path: &str) -> Result<(), String> {
    let pointer = match (parse(path), step.get("expect_error")) {
        (Ok(pointer), None) => pointer,
        (Err(_), Some(_)) => return Ok(()),
        (Ok(_), Some(_)) => return Err(format!("get {path} parsed, want an error")),
        (Err(e), None) => return Err(format!("get {path}: {e}")),
    };
    let got = model.get(&pointer);
    if step["expect_absent"] == true {
        // The suite counts a list slot left null by padding or removal as no
        // value. A key removed from an object must be gone.
        let in_list = pointer
            .parent()
            .is_some_and(|parent| model.get(&parent).is_some_and(Value::is_array));
        return match got {
            None => Ok(()),
            Some(Value::Null) if in_list => Ok(()),
            Some(value) => Err(format!("get {path} is {value}, want absent")),
        };
    }
    if let Some(kind) = step["expect_type"].as_str() {
        return match (kind, got) {
            ("list", Some(Value::Array(_))) | ("object", Some(Value::Object(_))) => Ok(()),
            _ => Err(format!("get {path} is {got:?}, want a {kind}")),
        };
    }
    let want = step.get("expect").ok_or("get step expects nothing")?;
    if got == Some(want) {
        Ok(())
    } else {
        Err(format!("get {path} is {got:?}, want {want}"))
    }
}

fn check_notified(
    watched: &HashMap<WatchId, Pointer>,
    fired: &[WatchId],
    expected: &Value,
) -> Result<(), String> {
    let mut got: Vec<String> = fired
        .iter()
        .map(|id| {
            watched
                .get(id)
                .map_or_else(|| format!("{id:?}"), ToString::to_string)
        })
        .collect();
    let mut want = expected
        .as_array()
        .ok_or("expect_notified is not a list")?
        .iter()
        .map(|path| parse(path.as_str().ok_or("path is not a string")?).map(|p| p.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    got.sort();
    want.sort();
    if got == want {
        Ok(())
    } else {
        Err(format!("notified {got:?}, want {want:?}"))
    }
}

/// Cases each catalog suite leaves out because they target a protocol version
/// before 1.0. A vendor refresh that changes them fails here, to be checked.
const FILTERED_CATALOG: usize = 26;
const FILTERED_COMMON_TYPES: usize = 10;
const FILTERED_AGENT_TO_RENDERER: usize = 3;

/// The protocol version a case targets: its own `protocolVersion`, else its
/// catalog's, else the spec's default for a catalog that declares none.
fn case_version(case: &Value) -> String {
    let declared = case
        .get("protocolVersion")
        .or_else(|| case.get("catalog")?.get("protocolVersion"))
        .cloned()
        .or_else(|| {
            let path = case.get("catalogPath")?.as_str()?;
            read_vendored(path).get("protocolVersion").cloned()
        });
    match declared {
        Some(Value::String(version)) => version,
        None => "0.9".to_owned(),
        Some(other) => panic!(
            "{}: protocolVersion {other} is not a string",
            case_name(case)
        ),
    }
}

/// Runs the cases of `file` that target v1.0, collecting failures, and
/// asserts how many it left out.
fn run_v10_suite(file: &str, filtered: usize, run: fn(&Value) -> Result<(), String>) {
    let mut failures = Vec::new();
    let mut left_out = 0;
    for case in load_suite(file) {
        let name = case_name(&case);
        match case_version(&case).as_str() {
            "1.0" => {}
            "0.9" | "v0.8" | "v0.9" | "v0.9.1" => {
                left_out += 1;
                continue;
            }
            other => panic!("{name}: unknown protocol version {other:?}"),
        }
        if is_skipped(name) {
            continue;
        }
        if let Err(reason) = run(&case) {
            failures.push(format!("{name}: {reason}"));
        }
    }
    assert_no_failures(file, &failures);
    assert_eq!(left_out, filtered, "{file}: cases left out as pre-1.0");
}

fn read_vendored(path: &str) -> Value {
    let full = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/a2ui")
        .join(path);
    let text = fs::read_to_string(&full).unwrap_or_else(|e| {
        panic!(
            "reading {}: {e} (is it in vendor/a2ui/FILES?)",
            full.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", full.display()))
}

/// Fails on a key of `object` that `known` does not list, so that a suite
/// refresh adding an expectation is not silently ignored.
fn check_keys(object: &Value, known: &[&str]) -> Result<(), String> {
    let map = object.as_object().ok_or("expectation is not an object")?;
    match map.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("unknown expectation key {key:?}")),
        None => Ok(()),
    }
}

/// `value` with `enum` and `required` lists sorted, which the suites compare
/// as sets. Object key order never matters to `Value` equality.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, child)| {
                    let mut child = canonical(child);
                    if let (true, Value::Array(items)) =
                        (key == "enum" || key == "required", &mut child)
                    {
                        items.sort_by_key(ToString::to_string);
                    }
                    (key.clone(), child)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn same(got: &Value, want: &Value, what: &str) -> Result<(), String> {
    if canonical(got) == canonical(want) {
        Ok(())
    } else {
        Err(format!("{what} differs:\n got: {got}\nwant: {want}"))
    }
}

/// The suite's error category for a catalog error. Every variant is listed,
/// so a new one needs a decision.
fn catalog_error_category(error: &CatalogError) -> &'static str {
    match error {
        CatalogError::InvalidMetadataKey { .. } => "ValidationError",
        CatalogError::NotAnObject
        | CatalogError::MissingId
        | CatalogError::InvalidField { .. }
        | CatalogError::UnsupportedVersion(_)
        | CatalogError::InvalidEntry { .. }
        | CatalogError::InvalidComponentName(_)
        | CatalogError::ReservedComponent
        | CatalogError::InvalidPropertyName { .. }
        | CatalogError::InvalidComposition { .. }
        | CatalogError::InvalidMetadata { .. }
        | CatalogError::InvalidFunctionName(_)
        | CatalogError::InvalidArgumentName { .. }
        | CatalogError::InvalidReturnType(_)
        | CatalogError::InvalidCallers(_)
        | CatalogError::InvalidActivation(_)
        | CatalogError::ActivationByAgent(_)
        | CatalogError::ExternalRef { .. }
        | CatalogError::UnresolvedRef { .. }
        | CatalogError::InvalidSchema { .. }
        | CatalogError::UnknownEntry { .. } => "CatalogError",
    }
}

/// Checks the outcome of a step that `expectError` may say must fail.
fn check_outcome<T>(case: &Value, outcome: Result<T, CatalogError>) -> Result<Option<T>, String> {
    match (outcome, case.get("expectError")) {
        (Ok(value), None) => Ok(Some(value)),
        (Err(error), Some(expected)) => {
            let got = catalog_error_category(&error);
            if expected["category"] == got {
                Ok(None)
            } else {
                Err(format!(
                    "failed with {got} ({error}), want {}",
                    expected["category"]
                ))
            }
        }
        (Ok(_), Some(expected)) => Err(format!("succeeded, want {}", expected["category"])),
        (Err(error), None) => Err(format!("failed: {error}")),
    }
}

/// The catalog document a case loads, with the id and version the case
/// supplies for a document that lacks them.
fn case_document(case: &Value) -> Value {
    let mut document = match (case.get("catalog"), case.get("catalogPath")) {
        (Some(inline), None) => inline.clone(),
        (None, Some(Value::String(path))) => read_vendored(path),
        _ => panic!("{}: needs one of catalog and catalogPath", case_name(case)),
    };
    let map = document
        .as_object_mut()
        .expect("a catalog document is an object");
    if let Some(id) = case.get("catalogId") {
        map.insert("catalogId".to_owned(), id.clone());
    }
    if let Some(version) = case.get("protocolVersion") {
        map.entry("protocolVersion")
            .or_insert_with(|| version.clone());
    }
    document
}

#[test]
fn catalog_suite() {
    run_v10_suite("catalog.yaml", FILTERED_CATALOG, |case| {
        match case["action"].as_str().ok_or("no action")? {
            "from_json" => run_from_json(case),
            "to_json" => run_to_json(case),
            "round_trip" => run_round_trip(case),
            "validation_schema" => run_validation_schema(case),
            "derive" => run_derive(case),
            other => Err(format!("unknown action {other:?}")),
        }
    });
}

fn run_from_json(case: &Value) -> Result<(), String> {
    let Some(catalog) = check_outcome(case, Catalog::from_json(case_document(case)))? else {
        return Ok(());
    };
    let Some(expect) = case.get("expect") else {
        return Ok(());
    };
    check_keys(
        expect,
        &[
            "catalogId",
            "protocolVersion",
            "components",
            "functions",
            "theme",
            "matchesBuiltinCatalog",
            "selfContained",
            "validComponents",
            "invalidComponents",
            "componentMetadata",
        ],
    )?;
    let document = catalog.to_json();
    if let Some(id) = expect.get("catalogId") {
        same(&Value::from(catalog.id()), id, "catalogId")?;
    }
    if let Some(version) = expect.get("protocolVersion") {
        same(
            &Value::from(catalog.protocol_version()),
            version,
            "protocolVersion",
        )?;
    }
    for kind in ["components", "functions", "theme"] {
        if let Some(want) = expect.get(kind) {
            check_entries(&document, kind, want)?;
        }
    }
    if expect["matchesBuiltinCatalog"] == true {
        check_matches_basic(&catalog)?;
    }
    if expect["selfContained"] == true {
        check_self_contained(&catalog.validation_schema())?;
    }
    if let Some(metadata) = expect.get("componentMetadata") {
        for (name, want) in metadata
            .as_object()
            .ok_or("componentMetadata is not a map")?
        {
            let def = catalog
                .component(name)
                .ok_or(format!("no component {name}"))?;
            let got = def.metadata().cloned().map_or(Value::Null, Value::Object);
            same(&got, want, &format!("{name} metadata"))?;
        }
    }
    let valid = expect.get("validComponents").and_then(Value::as_array);
    let invalid = expect.get("invalidComponents").and_then(Value::as_array);
    if valid.is_some() || invalid.is_some() {
        let validator = CatalogValidator::new(&catalog).map_err(|e| e.to_string())?;
        for instance in valid.into_iter().flatten() {
            validator
                .validate_component(instance)
                .map_err(|e| format!("{instance} is invalid: {e}"))?;
        }
        for instance in invalid.into_iter().flatten() {
            if validator.validate_component(instance).is_ok() {
                return Err(format!("{instance} is valid, want invalid"));
            }
        }
    }
    Ok(())
}

/// Checks a top-level part of a catalog document: for `components` and
/// `functions`, exactly the names `want` lists, or exactly its map of
/// definitions.
fn check_entries(document: &Value, kind: &str, want: &Value) -> Result<(), String> {
    let got = document.get(kind).cloned().unwrap_or(Value::Null);
    if kind == "theme" {
        return same(&got, want, kind);
    }
    let mut names: Vec<&String> = got
        .as_object()
        .map(|map| map.keys().collect())
        .unwrap_or_default();
    names.sort();
    match want {
        Value::Array(list) => {
            let mut wanted: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
            wanted.sort_unstable();
            if names
                .iter()
                .map(|name| name.as_str())
                .eq(wanted.iter().copied())
            {
                Ok(())
            } else {
                Err(format!("{kind} are {names:?}, want {wanted:?}"))
            }
        }
        Value::Object(_) => same(&got, want, kind),
        other => Err(format!(
            "{kind} expectation {other} is neither a list nor a map"
        )),
    }
}

/// The loaded catalog is equivalent to the crate's own basic catalog, as far
/// as declarations go: function implementations are not catalogs' business.
fn check_matches_basic(catalog: &Catalog) -> Result<(), String> {
    let basic = Catalog::basic();
    let same_declarations = catalog.id() == basic.id()
        && catalog.protocol_version() == basic.protocol_version()
        && catalog.components().len() == basic.components().len()
        && catalog.components().iter().all(|(name, def)| {
            basic.component(name).is_some_and(|own| {
                own.properties() == def.properties() && own.required() == def.required()
            })
        })
        && catalog.functions().len() == basic.functions().len()
        && catalog.functions().iter().all(|(name, def)| {
            basic.function(name).is_some_and(|own| {
                own.return_type() == def.return_type()
                    && own.callers() == def.callers()
                    && own.requires_user_activation() == def.requires_user_activation()
            })
        });
    if same_declarations {
        Ok(())
    } else {
        Err("differs from Catalog::basic()".to_owned())
    }
}

/// Every `$ref` in `schema` is local and resolves within it, and there is
/// at least one.
fn check_self_contained(schema: &Value) -> Result<(), String> {
    fn refs<'a>(node: &'a Value, found: &mut Vec<&'a str>) {
        match node {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref") {
                    found.push(reference);
                }
                map.values().for_each(|child| refs(child, found));
            }
            Value::Array(items) => items.iter().for_each(|child| refs(child, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    refs(schema, &mut found);
    if found.is_empty() {
        return Err("the validation schema has no $ref".to_owned());
    }
    for reference in found {
        let resolves = reference
            .strip_prefix('#')
            .is_some_and(|pointer| schema.pointer(pointer).is_some());
        if !resolves {
            return Err(format!(
                "$ref {reference:?} does not resolve within the validation schema"
            ));
        }
    }
    Ok(())
}

fn run_to_json(case: &Value) -> Result<(), String> {
    if case["builtinCatalog"] != "basic" {
        return Err(format!("unknown builtinCatalog {}", case["builtinCatalog"]));
    }
    let want = read_vendored(case["expectFile"].as_str().ok_or("no expectFile")?);
    same(&Catalog::basic().to_json(), &want, "toJson")
}

fn run_round_trip(case: &Value) -> Result<(), String> {
    let document = case_document(case);
    let catalog = Catalog::from_json(document.clone()).map_err(|e| e.to_string())?;
    let out = catalog.to_json();
    same(&out, &document, "toJson")?;
    let again = Catalog::from_json(out.clone()).map_err(|e| e.to_string())?;
    same(&again.to_json(), &out, "second toJson")
}

fn run_validation_schema(case: &Value) -> Result<(), String> {
    let catalog = Catalog::from_json(case_document(case)).map_err(|e| e.to_string())?;
    let want = read_vendored(case["expectFile"].as_str().ok_or("no expectFile")?);
    same(&catalog.validation_schema(), &want, "validationSchema")
}

fn run_derive(case: &Value) -> Result<(), String> {
    let source = Catalog::from_json(case_document(case)).map_err(|e| e.to_string())?;
    let args = &case["args"];
    check_keys(
        args,
        &["catalogId", "components", "functions", "instructions"],
    )?;
    let derivation = Derivation {
        catalog_id: args["catalogId"].as_str().ok_or("no catalogId")?.to_owned(),
        components: derive_entries(args.get("components"))?,
        functions: derive_entries(args.get("functions"))?,
        instructions: args
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    let Some(derived) = check_outcome(case, source.derive(derivation))? else {
        return Ok(());
    };
    let expect = case.get("expect").ok_or("no expect")?;
    check_keys(
        expect,
        &[
            "catalogId",
            "protocolVersion",
            "components",
            "functions",
            "theme",
            "instructions",
            "absentKeys",
        ],
    )?;
    let document = derived.to_json();
    for key in ["catalogId", "protocolVersion", "instructions"] {
        if let Some(want) = expect.get(key) {
            same(&document[key], want, key)?;
        }
    }
    for kind in ["components", "functions", "theme"] {
        if let Some(want) = expect.get(kind) {
            check_entries(&document, kind, want)?;
        }
    }
    for key in expect["absentKeys"].as_array().into_iter().flatten() {
        let key = key.as_str().ok_or("absentKeys holds a non-string")?;
        if document.get(key).is_some() {
            return Err(format!("toJson has {key:?}, want it absent"));
        }
    }
    Ok(())
}

/// `args.components` or `args.functions` of a derive case: a name keeps an
/// entry, a map with one key defines one.
fn derive_entries(entries: Option<&Value>) -> Result<Option<Vec<Entry>>, String> {
    let Some(entries) = entries else {
        return Ok(None);
    };
    let entries = entries.as_array().ok_or("derive entries are not a list")?;
    entries
        .iter()
        .map(|entry| match entry {
            Value::String(name) => Ok(Entry::Keep(name.clone())),
            Value::Object(map) if map.len() == 1 => {
                let (name, schema) = map.iter().next().expect("one key");
                Ok(Entry::Define {
                    name: name.clone(),
                    schema: schema.clone(),
                })
            }
            other => Err(format!(
                "derive entry {other} is neither a name nor a one-key map"
            )),
        })
        .collect::<Result<_, _>>()
        .map(Some)
}

#[test]
fn common_types_suite() {
    run_v10_suite(
        "common_types.yaml",
        FILTERED_COMMON_TYPES,
        |case| match case["action"].as_str().ok_or("no action")? {
            "common_types_schema" => {
                let want = read_vendored(case["expectFile"].as_str().ok_or("no expectFile")?);
                same(rho_a2ui::common_types(), &want, "common types")
            }
            "validate_common_type" => run_validate_common_type(case),
            other => Err(format!("unknown action {other:?}")),
        },
    );
}

fn run_validate_common_type(case: &Value) -> Result<(), String> {
    let definition = case["definition"].as_str().ok_or("no definition")?;
    let steps = case["steps"].as_array().ok_or("no steps")?;
    for (n, step) in steps.iter().enumerate() {
        let value = &step["value"];
        match (
            validate_common_type(definition, value),
            step.get("expectError"),
        ) {
            (Ok(()), None) => {}
            (Err(_), Some(expected)) if expected["category"] == "ValidationError" => {}
            (Err(e), None) => return Err(format!("step {n}: {value} is invalid: {e}")),
            (outcome, Some(expected)) => {
                return Err(format!(
                    "step {n}: {value} gave {outcome:?}, want {expected}"
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn agent_to_renderer_suite() {
    run_v10_suite(
        "agent_to_renderer.yaml",
        FILTERED_AGENT_TO_RENDERER,
        |case| match case["action"].as_str().ok_or("no action")? {
            "agent_to_renderer_schema" => {
                let want = read_vendored(case["expectFile"].as_str().ok_or("no expectFile")?);
                same(
                    rho_a2ui::agent_to_renderer(),
                    &want,
                    "agent_to_renderer schema",
                )
            }
            other => Err(format!("unknown action {other:?}")),
        },
    );
}
